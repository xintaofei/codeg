use std::sync::Arc;

use axum::{
    extract::Extension,
    http::{StatusCode, Uri},
    middleware::{self, Next},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

use super::{auth, handlers, ws};
use crate::app_state::AppState;

pub fn build_router(state: Arc<AppState>, token: String, static_dir: std::path::PathBuf) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let token_for_ws = token.clone();

    let api = Router::new()
        .route("/health", post(health_check))
        // ─── Conversations ───
        .route("/list_conversations", post(handlers::conversations::list_conversations))
        .route("/get_conversation", post(handlers::conversations::get_conversation))
        .route("/list_folder_conversations", post(handlers::conversations::list_folder_conversations))
        .route("/get_folder_conversation", post(handlers::conversations::get_folder_conversation))
        .route("/import_local_conversations", post(handlers::conversations::import_local_conversations))
        .route("/list_folders", post(handlers::conversations::list_folders))
        .route("/get_stats", post(handlers::conversations::get_stats))
        .route("/get_sidebar_data", post(handlers::conversations::get_sidebar_data))
        .route("/create_conversation", post(handlers::conversations::create_conversation))
        .route("/update_conversation_status", post(handlers::conversations::update_conversation_status))
        .route("/update_conversation_title", post(handlers::conversations::update_conversation_title))
        .route("/delete_conversation", post(handlers::conversations::delete_conversation))
        .route("/update_conversation_external_id", post(handlers::conversations::update_conversation_external_id))
        // ─── Folders ───
        .route("/load_folder_history", post(handlers::folders::load_folder_history))
        .route("/list_open_folders", post(handlers::folders::list_open_folders))
        .route("/close_folder_window", post(handlers::folders::close_folder_window))
        .route("/get_folder", post(handlers::folders::get_folder))
        .route("/open_folder_window", post(handlers::folders::open_folder_window))
        .route("/add_folder_to_history", post(handlers::folders::add_folder_to_history))
        .route("/set_folder_parent_branch", post(handlers::folders::set_folder_parent_branch))
        .route("/remove_folder_from_history", post(handlers::folders::remove_folder_from_history))
        .route("/create_folder_directory", post(handlers::folders::create_folder_directory))
        .route("/save_folder_opened_conversations", post(handlers::folders::save_folder_opened_conversations))
        .route("/get_git_branch", post(handlers::folders::get_git_branch))
        .route("/get_home_directory", post(handlers::folders::get_home_directory))
        .route("/list_directory_entries", post(handlers::folders::list_directory_entries))
        .route("/get_file_tree", post(handlers::folders::get_file_tree))
        .route("/start_file_tree_watch", post(handlers::folders::start_file_tree_watch))
        .route("/stop_file_tree_watch", post(handlers::folders::stop_file_tree_watch))
        // ─── Window navigation ───
        .route("/open_settings_window", post(handlers::folders::open_settings_window))
        .route("/open_commit_window", post(handlers::folders::open_commit_window))
        .route("/open_merge_window", post(handlers::folders::open_merge_window))
        .route("/open_stash_window", post(handlers::folders::open_stash_window))
        .route("/open_push_window", post(handlers::folders::open_push_window))
        // ─── Git (pure) ───
        .route("/git_status", post(handlers::git::git_status))
        .route("/git_init", post(handlers::git::git_init))
        .route("/git_log", post(handlers::git::git_log))
        .route("/git_list_all_branches", post(handlers::git::git_list_all_branches))
        .route("/git_list_branches", post(handlers::git::git_list_branches))
        .route("/git_commit_branches", post(handlers::git::git_commit_branches))
        .route("/git_show_file", post(handlers::git::git_show_file))
        .route("/git_diff", post(handlers::git::git_diff))
        .route("/git_diff_with_branch", post(handlers::git::git_diff_with_branch))
        .route("/git_show_diff", post(handlers::git::git_show_diff))
        .route("/git_list_remotes", post(handlers::git::git_list_remotes))
        .route("/git_add_remote", post(handlers::git::git_add_remote))
        .route("/git_remove_remote", post(handlers::git::git_remove_remote))
        .route("/git_set_remote_url", post(handlers::git::git_set_remote_url))
        .route("/git_new_branch", post(handlers::git::git_new_branch))
        .route("/git_checkout", post(handlers::git::git_checkout))
        .route("/git_delete_branch", post(handlers::git::git_delete_branch))
        .route("/git_merge", post(handlers::git::git_merge))
        .route("/git_rebase", post(handlers::git::git_rebase))
        .route("/git_worktree_add", post(handlers::git::git_worktree_add))
        .route("/git_push_info", post(handlers::git::git_push_info))
        .route("/git_start_pull_merge", post(handlers::git::git_start_pull_merge))
        .route("/git_has_merge_head", post(handlers::git::git_has_merge_head))
        .route("/git_is_tracked", post(handlers::git::git_is_tracked))
        .route("/git_rollback_file", post(handlers::git::git_rollback_file))
        .route("/git_add_files", post(handlers::git::git_add_files))
        .route("/git_list_conflicts", post(handlers::git::git_list_conflicts))
        .route("/git_conflict_file_versions", post(handlers::git::git_conflict_file_versions))
        .route("/git_resolve_conflict", post(handlers::git::git_resolve_conflict))
        .route("/git_abort_operation", post(handlers::git::git_abort_operation))
        .route("/git_continue_operation", post(handlers::git::git_continue_operation))
        .route("/git_stash_push", post(handlers::git::git_stash_push))
        .route("/git_stash_pop", post(handlers::git::git_stash_pop))
        .route("/git_stash_list", post(handlers::git::git_stash_list))
        .route("/git_stash_apply", post(handlers::git::git_stash_apply))
        .route("/git_stash_drop", post(handlers::git::git_stash_drop))
        .route("/git_stash_clear", post(handlers::git::git_stash_clear))
        .route("/git_stash_show", post(handlers::git::git_stash_show))
        // ─── Git (remote) ───
        .route("/git_pull", post(handlers::git::git_pull))
        .route("/git_push", post(handlers::git::git_push))
        .route("/git_fetch", post(handlers::git::git_fetch))
        .route("/git_commit", post(handlers::git::git_commit))
        .route("/git_fetch_remote", post(handlers::git::git_fetch_remote))
        .route("/clone_repository", post(handlers::git::clone_repository))
        // ─── Files ───
        .route("/read_file_preview", post(handlers::files::read_file_preview))
        .route("/read_file_base64", post(handlers::files::read_file_base64))
        .route("/read_file_for_edit", post(handlers::files::read_file_for_edit))
        .route("/save_file_content", post(handlers::files::save_file_content))
        .route("/save_file_copy", post(handlers::files::save_file_copy))
        .route("/rename_file_tree_entry", post(handlers::files::rename_file_tree_entry))
        .route("/delete_file_tree_entry", post(handlers::files::delete_file_tree_entry))
        .route("/create_file_tree_entry", post(handlers::files::create_file_tree_entry))
        // ─── Folder commands ───
        .route("/list_folder_commands", post(handlers::folder_commands::list_folder_commands))
        .route("/create_folder_command", post(handlers::folder_commands::create_folder_command))
        .route("/update_folder_command", post(handlers::folder_commands::update_folder_command))
        .route("/delete_folder_command", post(handlers::folder_commands::delete_folder_command))
        .route("/reorder_folder_commands", post(handlers::folder_commands::reorder_folder_commands))
        .route("/bootstrap_folder_commands_from_package_json", post(handlers::folder_commands::bootstrap_folder_commands_from_package_json))
        // ─── MCP ───
        .route("/mcp_scan_local", post(handlers::mcp::mcp_scan_local))
        .route("/mcp_list_marketplaces", post(handlers::mcp::mcp_list_marketplaces))
        .route("/mcp_search_marketplace", post(handlers::mcp::mcp_search_marketplace))
        .route("/mcp_get_marketplace_server_detail", post(handlers::mcp::mcp_get_marketplace_server_detail))
        .route("/mcp_install_from_marketplace", post(handlers::mcp::mcp_install_from_marketplace))
        .route("/mcp_upsert_local_server", post(handlers::mcp::mcp_upsert_local_server))
        .route("/mcp_set_server_apps", post(handlers::mcp::mcp_set_server_apps))
        .route("/mcp_remove_server", post(handlers::mcp::mcp_remove_server))
        // ─── Version control settings ───
        .route("/detect_git", post(handlers::version_control::detect_git))
        .route("/test_git_path", post(handlers::version_control::test_git_path))
        .route("/get_git_settings", post(handlers::version_control::get_git_settings))
        .route("/update_git_settings", post(handlers::version_control::update_git_settings))
        .route("/get_github_accounts", post(handlers::version_control::get_github_accounts))
        .route("/update_github_accounts", post(handlers::version_control::update_github_accounts))
        .route("/validate_github_token", post(handlers::version_control::validate_github_token))
        .route("/save_account_token", post(handlers::version_control::save_account_token))
        .route("/get_account_token", post(handlers::version_control::get_account_token))
        .route("/delete_account_token", post(handlers::version_control::delete_account_token))
        // ─── System settings ───
        .route("/get_system_proxy_settings", post(handlers::system_settings::get_system_proxy_settings))
        .route("/get_system_language_settings", post(handlers::system_settings::get_system_language_settings))
        .route("/update_system_proxy_settings", post(handlers::system_settings::update_system_proxy_settings))
        .route("/update_system_language_settings", post(handlers::system_settings::update_system_language_settings))
        // ─── ACP ───
        .route("/acp_get_agent_status", post(handlers::acp::acp_get_agent_status))
        .route("/acp_list_agents", post(handlers::acp::acp_list_agents))
        .route("/acp_connect", post(handlers::acp::acp_connect))
        .route("/acp_disconnect", post(handlers::acp::acp_disconnect))
        .route("/acp_prompt", post(handlers::acp::acp_prompt))
        .route("/acp_preflight", post(handlers::acp::acp_preflight))
        .route("/acp_set_mode", post(handlers::acp::acp_set_mode))
        .route("/acp_set_config_option", post(handlers::acp::acp_set_config_option))
        .route("/acp_cancel", post(handlers::acp::acp_cancel))
        .route("/acp_fork", post(handlers::acp::acp_fork))
        .route("/acp_respond_permission", post(handlers::acp::acp_respond_permission))
        .route("/acp_list_connections", post(handlers::acp::acp_list_connections))
        .route("/acp_clear_binary_cache", post(handlers::acp::acp_clear_binary_cache))
        .route("/acp_update_agent_preferences", post(handlers::acp::acp_update_agent_preferences))
        .route("/acp_download_agent_binary", post(handlers::acp::acp_download_agent_binary))
        .route("/acp_detect_agent_local_version", post(handlers::acp::acp_detect_agent_local_version))
        .route("/acp_prepare_npx_agent", post(handlers::acp::acp_prepare_npx_agent))
        .route("/acp_uninstall_agent", post(handlers::acp::acp_uninstall_agent))
        .route("/acp_reorder_agents", post(handlers::acp::acp_reorder_agents))
        .route("/acp_list_agent_skills", post(handlers::acp::acp_list_agent_skills))
        .route("/acp_read_agent_skill", post(handlers::acp::acp_read_agent_skill))
        .route("/acp_save_agent_skill", post(handlers::acp::acp_save_agent_skill))
        .route("/acp_delete_agent_skill", post(handlers::acp::acp_delete_agent_skill))
        // ─── Project boot ───
        .route("/detect_package_manager", post(handlers::project_boot::detect_package_manager))
        .route("/create_shadcn_project", post(handlers::project_boot::create_shadcn_project))
        // ─── Web Server ───
        .route("/get_web_server_status", post(handlers::web_server::get_web_server_status))
        .route("/start_web_server", post(handlers::web_server::start_web_server))
        .route("/stop_web_server", post(handlers::web_server::stop_web_server))
        .route("/check_app_update", post(handlers::web_server::check_app_update))
        // ─── Chat Channels ───
        .route("/list_chat_channels", post(handlers::chat_channel::list_chat_channels))
        .route("/create_chat_channel", post(handlers::chat_channel::create_chat_channel))
        .route("/update_chat_channel", post(handlers::chat_channel::update_chat_channel))
        .route("/delete_chat_channel", post(handlers::chat_channel::delete_chat_channel))
        .route("/save_chat_channel_token", post(handlers::chat_channel::save_chat_channel_token))
        .route("/get_chat_channel_has_token", post(handlers::chat_channel::get_chat_channel_has_token))
        .route("/delete_chat_channel_token", post(handlers::chat_channel::delete_chat_channel_token))
        .route("/connect_chat_channel", post(handlers::chat_channel::connect_chat_channel))
        .route("/disconnect_chat_channel", post(handlers::chat_channel::disconnect_chat_channel))
        .route("/test_chat_channel", post(handlers::chat_channel::test_chat_channel))
        .route("/get_chat_channel_status", post(handlers::chat_channel::get_chat_channel_status))
        .route("/list_chat_channel_messages", post(handlers::chat_channel::list_chat_channel_messages))
        .route("/get_chat_command_prefix", post(handlers::chat_channel::get_chat_command_prefix))
        .route("/set_chat_command_prefix", post(handlers::chat_channel::set_chat_command_prefix))
        .route("/get_chat_event_filter", post(handlers::chat_channel::get_chat_event_filter))
        .route("/set_chat_event_filter", post(handlers::chat_channel::set_chat_event_filter))
        // ─── Terminal ───
        .route("/terminal_spawn", post(handlers::terminal::terminal_spawn))
        .route("/terminal_write", post(handlers::terminal::terminal_write))
        .route("/terminal_resize", post(handlers::terminal::terminal_resize))
        .route("/terminal_kill", post(handlers::terminal::terminal_kill))
        .route("/terminal_list", post(handlers::terminal::terminal_list))
        // Catch-all
        .fallback(api_not_found)
        .layer(middleware::from_fn(move |req, next| {
            auth::require_token(req, next, token.clone())
        }));

    // WebSocket route (auth via query param)
    let ws_route = Router::new()
        .route("/ws/events", get(ws::ws_handler))
        .layer(middleware::from_fn(move |req, next| {
            auth::require_token(req, next, token_for_ws.clone())
        }));

    // Static file serving.
    // Next.js static export produces "folder.html" for "/folder" route.
    // We use a middleware to rewrite "/folder" → "/folder.html" before ServeDir.
    let fallback = ServeDir::new(&static_dir)
        .fallback(ServeFile::new(static_dir.join("index.html")));

    let static_dir_for_mw = static_dir.clone();
    let html_rewrite = middleware::from_fn(move |req: axum::extract::Request, next: Next| {
        let dir = static_dir_for_mw.clone();
        async move {
            let path = req.uri().path();
            // If path has no extension (not a file) and a .html version exists, rewrite
            if path != "/" && !path.contains('.') && !path.starts_with("/api") && !path.starts_with("/ws") {
                let html_path = format!("{}.html", path.trim_end_matches('/'));
                let html_file = dir.join(html_path.trim_start_matches('/'));
                if html_file.exists() {
                    // Rebuild URI with .html suffix preserving query string
                    let new_path = if let Some(q) = req.uri().query() {
                        format!("{}?{}", html_path, q)
                    } else {
                        html_path
                    };
                    if let Ok(new_uri) = new_path.parse::<Uri>() {
                        let (mut parts, body) = req.into_parts();
                        parts.uri = new_uri;
                        let req = axum::extract::Request::from_parts(parts, body);
                        return next.run(req).await;
                    }
                }
            }
            next.run(req).await
        }
    });

    Router::new()
        .nest("/api", api)
        .merge(ws_route)
        .fallback_service(fallback)
        .layer(html_rewrite)
        .layer(cors)
        .layer(Extension(state))
}

async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn api_not_found(uri: axum::http::Uri) -> impl IntoResponse {
    let command = uri.path().trim_start_matches('/');
    eprintln!("[WEB] Unimplemented API endpoint: {}", command);
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "code": "not_implemented",
            "message": format!("API endpoint '{}' is not available in web mode", command),
        })),
    )
}
