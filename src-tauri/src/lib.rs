mod acp;
pub use acp::{idle_sweep_task, idle_timeout_from_env, lifecycle_subscriber_task, SWEEP_INTERVAL_SECS};
mod app_error;
pub mod app_state;
pub mod chat_channel;
pub mod commands;
pub mod db;
pub mod git_credential;
pub mod git_repo;
pub mod keyring_store;
mod models;
mod network;
mod parsers;
pub mod paths;
pub mod pet_state_mapper;
pub mod pets;
#[cfg(feature = "tauri-runtime")]
pub mod preferences;
pub mod process;
mod terminal;
pub mod web;
pub mod workspace_state;

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
        acp as acp_commands, chat_channel as chat_channel_commands, conversations,
        experts as experts_commands, file_io, folder_commands, folders, mcp as mcp_commands,
        model_provider as model_provider_commands, notification, pet as pet_commands,
        project_boot, quick_messages as quick_messages_commands, system_settings,
        terminal as terminal_commands, version_control, windows,
        workspace_state as workspace_state_commands,
    };
    use crate::terminal::manager::TerminalManager;
    use crate::{db, network, process, web};
    use tauri::Manager;

    static APP_QUITTING: AtomicBool = AtomicBool::new(false);

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
            Ok(prev) => prev
                .split_whitespace()
                .map(str::to_string)
                .collect(),
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

        tauri::Builder::default()
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
            .manage(std::sync::Arc::new(
                web::event_bridge::WebEventBroadcaster::new(),
            ))
            .setup(|app| {
                let app_data_dir = app.path().app_data_dir()?;
                let app_version = env!("CARGO_PKG_VERSION");
                let database =
                    tauri::async_runtime::block_on(db::init_database(&app_data_dir, app_version))
                        .map_err(|e| e.to_string())?;
                app.manage(database);

                // Restore and apply saved system proxy settings before any network operation.
                let db = app.state::<db::AppDatabase>();
                match tauri::async_runtime::block_on(system_settings::load_system_proxy_settings(
                    &db.conn,
                )) {
                    Ok(settings) => {
                        let _ = network::proxy::apply_system_proxy_settings(&settings);
                    }
                    Err(err) => {
                        eprintln!("[Settings] failed to load system proxy settings: {err}");
                    }
                }

                // Load saved appearance settings before any window is created.
                tauri::async_runtime::block_on(windows::load_saved_zoom(&db.conn));
                tauri::async_runtime::block_on(windows::load_saved_appearance_mode(&db.conn));

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
                    let cm = app.state::<ConnectionManager>().clone_ref();
                    let emitter = web::event_bridge::EventEmitter::Tauri(app.handle().clone());
                    tauri::async_runtime::spawn(async move {
                        ccm_ref.start_background(br, db_conn, cm, emitter).await;
                    });
                }

                // Spawn the desktop pet state mapper: subscribes to ACP events
                // and emits `pet://state` whenever the aggregated pet state
                // changes. The renderer in the floating pet window listens
                // for these events to drive its sprite animation row.
                {
                    let broadcaster = app
                        .state::<std::sync::Arc<web::event_bridge::WebEventBroadcaster>>()
                        .inner()
                        .clone();
                    let emitter = web::event_bridge::EventEmitter::Tauri(app.handle().clone());
                    tauri::async_runtime::spawn(crate::pet_state_mapper::pet_state_subscriber_task(
                        broadcaster,
                        emitter,
                    ));
                }

                // Spawn the LifecycleSubscriber: persists cross-connection DB state
                // (currently `external_id` on conversation rows when SessionStarted fires)
                // off the emit hot path. `subscribe()` runs synchronously inside
                // `lifecycle_subscriber_task` before the future is returned, so the
                // subscribe-before-spawn invariant holds. The setup callback runs
                // outside any tokio runtime, so we use `tauri::async_runtime::spawn`.
                {
                    let db_conn = app.state::<db::AppDatabase>().conn.clone();
                    let cm = app.state::<ConnectionManager>().clone_ref();
                    let broadcaster = app
                        .state::<std::sync::Arc<web::event_bridge::WebEventBroadcaster>>()
                        .inner()
                        .clone();
                    tauri::async_runtime::spawn(crate::acp::lifecycle_subscriber_task(
                        db_conn,
                        cm,
                        broadcaster,
                    ));
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
                        .min_inner_size(900.0, 600.0);
                    if let Ok(w) = windows::apply_platform_window_style(builder).build() {
                        windows::post_window_setup(&w);
                    }
                }

                Ok(())
            })
            .on_window_event(|window, event| {
                let label = window.label().to_string();

                if label == "settings"
                    && matches!(
                        event,
                        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                    )
                {
                    let app = window.app_handle();
                    if let Some(state) = app.try_state::<windows::SettingsWindowState>() {
                        windows::restore_windows_after_settings(app, &state);
                    }
                }

                if label.starts_with("commit-")
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

                if label.starts_with("merge-")
                    && matches!(
                        event,
                        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                    )
                {
                    let app = window.app_handle();
                    if let Some(state) = app.try_state::<windows::MergeWindowState>() {
                        windows::restore_window_after_merge(app, &state, &label);
                    }
                    let app_clone = window.app_handle().clone();
                    let label_clone = label.clone();
                    tauri::async_runtime::spawn(async move {
                        windows::cleanup_dangling_merge(&app_clone, &label_clone).await;
                    });
                }

                if label == "pet"
                    && matches!(
                        event,
                        tauri::WindowEvent::CloseRequested { .. }
                            | tauri::WindowEvent::Destroyed
                    )
                {
                    // Persist `enabled = false` so the next launch doesn't
                    // race-open the pet before the user asks for it. We
                    // intentionally do NOT clear `active_pet_id` — the user
                    // chose that pet, they want it back next time they open
                    // the window.
                    if let Some(db) = window.app_handle().try_state::<db::AppDatabase>() {
                        let conn = db.conn.clone();
                        tauri::async_runtime::spawn(async move {
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
                        });
                    }
                }

                if label == "main" && matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                    let app = window.app_handle();
                    if let Some(cm) = app.try_state::<ConnectionManager>() {
                        let disconnected =
                            tauri::async_runtime::block_on(cm.disconnect_by_owner_window(&label));
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
            })
            .invoke_handler(tauri::generate_handler![
                conversations::list_conversations,
                conversations::get_conversation,
                conversations::list_all_conversations,
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
                conversations::delete_conversation,
                folders::load_folder_history,
                folders::get_folder,
                folders::list_open_folder_details,
                folders::list_all_folder_details,
                folders::open_folder,
                folders::open_folder_by_id,
                folders::remove_folder_from_workspace,
                folders::reorder_folders,
                folders::update_folder_color,
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
                folders::get_file_tree,
                folders::read_file_base64,
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
                windows::open_pet_window,
                windows::close_pet_window,
                windows::pet_window_record_position,
                windows::update_traffic_light_position,
                windows::update_appearance_mode,
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
                project_boot::detect_package_manager,
                project_boot::create_shadcn_project,
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
                acp_commands::acp_cancel,
                acp_commands::acp_fork,
                acp_commands::acp_respond_permission,
                acp_commands::acp_disconnect,
                acp_commands::acp_touch_connection,
                acp_commands::acp_list_connections,
                acp_commands::acp_get_session_snapshot,
                acp_commands::acp_get_session_snapshot_by_conversation,
                acp_commands::acp_list_agents,
                acp_commands::acp_get_agent_status,
                acp_commands::acp_clear_binary_cache,
                acp_commands::acp_download_agent_binary,
                acp_commands::acp_detect_agent_local_version,
                acp_commands::acp_prepare_npx_agent,
                acp_commands::acp_uninstall_agent,
                acp_commands::acp_update_agent_preferences,
                acp_commands::acp_update_agent_env,
                acp_commands::acp_update_agent_config,
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
                web::probe_web_service_port,
            ])
            .build(tauri::generate_context!())
            .expect("error while building tauri application")
            .run(|app, event| {
                if let tauri::RunEvent::ExitRequested { .. } = event {
                    APP_QUITTING.store(true, Ordering::Relaxed);
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
            });
    }
}

#[cfg(feature = "tauri-runtime")]
pub use tauri_app::run;
