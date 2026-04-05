mod acp;
mod app_error;
pub mod app_state;
pub mod chat_channel;
mod commands;
pub mod db;
pub mod git_credential;
pub mod keyring_store;
mod models;
mod network;
mod parsers;
pub mod process;
mod terminal;
pub mod web;

#[cfg(feature = "tauri-runtime")]
mod tauri_app {
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::acp::manager::ConnectionManager;
    use crate::chat_channel::manager::ChatChannelManager;
    use crate::commands::{
        acp as acp_commands, chat_channel as chat_channel_commands, conversations, folder_commands,
        folders, mcp as mcp_commands, model_provider as model_provider_commands, notification,
        project_boot, system_settings,
        terminal as terminal_commands, version_control, windows,
    };
    use crate::terminal::manager::TerminalManager;
    use crate::{db, network, process, web};
    use tauri::Manager;

    static APP_QUITTING: AtomicBool = AtomicBool::new(false);

    fn get_folder_id_from_url(window: &tauri::Window) -> Option<i32> {
        let webview = window.get_webview_window(window.label())?;
        let url = webview.url().ok()?;
        url.query_pairs()
            .find(|(key, _)| key == "id")
            .and_then(|(_, value)| value.parse::<i32>().ok())
    }

    #[cfg_attr(mobile, tauri::mobile_entry_point)]
    pub fn run() {
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
            .manage(std::sync::Arc::new(web::event_bridge::WebEventBroadcaster::new()))
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

                // Start chat channel background tasks
                {
                    let ccm = app.state::<ChatChannelManager>();
                    let broadcaster = app
                        .state::<std::sync::Arc<web::event_bridge::WebEventBroadcaster>>();
                    let db_conn = app.state::<db::AppDatabase>().conn.clone();
                    let ccm_ref = ccm.clone_ref();
                    let br = broadcaster.inner().clone();
                    let cm = app.state::<ConnectionManager>().clone_ref();
                    let emitter = web::event_bridge::EventEmitter::Tauri(app.handle().clone());
                    tauri::async_runtime::spawn(async move {
                        ccm_ref.start_background(br, db_conn, cm, emitter).await;
                    });
                }

                // Restore previously open folders or show welcome
                let db = app.state::<db::AppDatabase>();
                let open_folders = tauri::async_runtime::block_on(
                    db::service::folder_service::list_open_folders(&db.conn),
                )
                .unwrap_or_default();

                if open_folders.is_empty() {
                    let _ = windows::open_welcome_window(app.handle());
                } else {
                    for entry in &open_folders {
                        let label = windows::folder_window_label(entry.id);
                        let url = tauri::WebviewUrl::App(format!("folder?id={}", entry.id).into());
                        let builder = tauri::WebviewWindowBuilder::new(app, &label, url)
                            .title(&entry.name)
                            .inner_size(1260.0, 860.0)
                            .min_inner_size(900.0, 600.0);
                        let _ = windows::apply_platform_window_style(builder).build();
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

                if label == "project-boot"
                    && matches!(
                        event,
                        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                    )
                {
                    let app = window.app_handle();
                    if !APP_QUITTING.load(Ordering::Relaxed) {
                        let has_other = app
                            .webview_windows()
                            .keys()
                            .any(|l| *l != label && *l != "settings");
                        if !has_other {
                            let _ = windows::open_welcome_window(app);
                        }
                    }
                }

                if let tauri::WindowEvent::CloseRequested { .. } = event {
                    if label.starts_with("folder-") {
                        let app = window.app_handle();
                        if let Some(cm) = app.try_state::<ConnectionManager>() {
                            let disconnected =
                                tauri::async_runtime::block_on(cm.disconnect_by_owner_window(&label));
                            eprintln!(
                                "[ACP] folder window closing label={} disconnected_connections={}",
                                label, disconnected
                            );
                        }

                        if !APP_QUITTING.load(Ordering::Relaxed) {
                            if let Some(folder_id) = get_folder_id_from_url(window) {
                                if let Some(db) = app.try_state::<db::AppDatabase>() {
                                    let _ = tauri::async_runtime::block_on(
                                        db::service::folder_service::set_folder_open(
                                            &db.conn, folder_id, false,
                                        ),
                                    );
                                }
                            }
                        }

                        if let Some(tm) = app.try_state::<TerminalManager>() {
                            let killed = tm.kill_by_owner_window(&label);
                            eprintln!(
                                "[TERM] folder window closing label={} killed_terminals={}",
                                label, killed
                            );
                        }
                        let has_other_folder = app
                            .webview_windows()
                            .keys()
                            .any(|l| l.starts_with("folder-") && *l != label);
                        if !has_other_folder && !APP_QUITTING.load(Ordering::Relaxed) {
                            let _ = windows::open_welcome_window(app);
                        }
                    }
                }
            })
            .invoke_handler(tauri::generate_handler![
                conversations::list_conversations,
                conversations::get_conversation,
                conversations::list_folder_conversations,
                conversations::import_local_conversations,
                conversations::get_folder_conversation,
                conversations::list_folders,
                conversations::get_stats,
                conversations::get_sidebar_data,
                conversations::create_conversation,
                conversations::update_conversation_status,
                conversations::update_conversation_title,
                conversations::update_conversation_external_id,
                conversations::delete_conversation,
                folders::load_folder_history,
                folders::get_folder,
                folders::add_folder_to_history,
                folders::set_folder_parent_branch,
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
                folders::save_folder_opened_conversations,
                folders::start_file_tree_watch,
                folders::stop_file_tree_watch,
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
                windows::list_open_folders,
                windows::focus_folder_window,
                windows::open_merge_window,
                windows::open_stash_window,
                windows::open_push_window,
                windows::open_project_boot_window,
                project_boot::detect_package_manager,
                project_boot::create_shadcn_project,
                system_settings::get_system_proxy_settings,
                system_settings::update_system_proxy_settings,
                system_settings::get_system_language_settings,
                system_settings::update_system_language_settings,
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
                acp_commands::acp_list_connections,
                acp_commands::acp_list_agents,
                acp_commands::acp_get_agent_status,
                acp_commands::acp_clear_binary_cache,
                acp_commands::acp_download_agent_binary,
                acp_commands::acp_detect_agent_local_version,
                acp_commands::acp_prepare_npx_agent,
                acp_commands::acp_uninstall_agent,
                acp_commands::acp_update_agent_preferences,
                acp_commands::acp_reorder_agents,
                acp_commands::acp_list_agent_skills,
                acp_commands::acp_read_agent_skill,
                acp_commands::acp_save_agent_skill,
                acp_commands::acp_delete_agent_skill,
                folder_commands::list_folder_commands,
                folder_commands::create_folder_command,
                folder_commands::update_folder_command,
                folder_commands::delete_folder_command,
                folder_commands::reorder_folder_commands,
                folder_commands::bootstrap_folder_commands_from_package_json,
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
            ])
            .build(tauri::generate_context!())
            .expect("error while building tauri application")
            .run(|app, event| {
                if let tauri::RunEvent::ExitRequested { .. } = event {
                    APP_QUITTING.store(true, Ordering::Relaxed);
                    if let Some(ws) = app.try_state::<web::WebServerState>() {
                        web::do_stop_web_server(&ws);
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
