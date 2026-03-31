use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sea_orm::DatabaseConnection;
use tokio::task::JoinHandle;

use super::manager::ChatChannelManager;
use super::message_formatter;
use super::types::RichMessage;
use crate::db::service::{app_metadata_service, chat_channel_message_log_service, chat_channel_service};
use crate::web::event_bridge::WebEventBroadcaster;

/// Minimum interval between pushes for the same event type per channel (debounce).
const DEBOUNCE_SECS: u64 = 5;

pub fn spawn_event_subscriber(
    broadcaster: Arc<WebEventBroadcaster>,
    manager: ChatChannelManager,
    db_conn: DatabaseConnection,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = broadcaster.subscribe();
        let mut last_push: HashMap<(i32, String), Instant> = HashMap::new();

        loop {
            let event = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!("[ChatChannel] event subscriber lagged by {n} messages");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    eprintln!("[ChatChannel] event broadcaster closed, stopping subscriber");
                    break;
                }
            };

            let message = match parse_event(&event.channel, &event.payload) {
                Some((event_type, msg)) => {
                    // Global event filter check
                    let global_filter = app_metadata_service::get_value(&db_conn, "chat_event_filter")
                        .await
                        .ok()
                        .flatten()
                        .and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok());

                    if let Some(filter) = &global_filter {
                        if !filter.contains(&event_type) {
                            continue;
                        }
                    }

                    // Check enabled channels and forward
                    let channels = match chat_channel_service::list_enabled(&db_conn).await {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("[ChatChannel] failed to list channels: {e}");
                            continue;
                        }
                    };

                    for ch in &channels {
                        // Debounce
                        let key = (ch.id, event_type.clone());
                        let now = Instant::now();
                        if let Some(last) = last_push.get(&key) {
                            if now.duration_since(*last) < Duration::from_secs(DEBOUNCE_SECS) {
                                continue;
                            }
                        }
                        last_push.insert(key, now);

                        // Send
                        let send_result = manager.send_to_channel(ch.id, &msg).await;
                        let (status, error_detail) = match &send_result {
                            Ok(_) => ("sent", None),
                            Err(e) => ("failed", Some(e.to_string())),
                        };

                        let _ = chat_channel_message_log_service::create_log(
                            &db_conn,
                            ch.id,
                            "outbound",
                            "event_push",
                            &msg.to_plain_text(),
                            status,
                            error_detail,
                        )
                        .await;
                    }

                    Some(msg)
                }
                None => None,
            };

            drop(message);
        }
    })
}

fn parse_event(channel: &str, payload: &serde_json::Value) -> Option<(String, RichMessage)> {
    match channel {
        "acp://event" => parse_acp_event(payload),
        "folder://git-push-succeeded" => parse_git_push(payload),
        "folder://git-commit-succeeded" => parse_git_commit(payload),
        _ => None,
    }
}

fn parse_acp_event(payload: &serde_json::Value) -> Option<(String, RichMessage)> {
    let event_type = payload.get("type")?.as_str()?;
    let connection_id = payload
        .get("connection_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    match event_type {
        "session_started" => {
            let agent_type = payload
                .pointer("/data/agent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown Agent");
            let folder = payload
                .pointer("/data/folder_name")
                .and_then(|v| v.as_str())
                .unwrap_or(connection_id);
            Some((
                "session_started".to_string(),
                message_formatter::format_session_started(agent_type, folder),
            ))
        }
        "turn_complete" => {
            let stop_reason = payload
                .pointer("/data/stop_reason")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            // Only push for end_turn, not for intermediate completions
            if stop_reason != "end_turn" {
                return None;
            }
            let agent_type = payload
                .pointer("/data/agent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown Agent");
            Some((
                "turn_complete".to_string(),
                message_formatter::format_turn_complete(agent_type, stop_reason),
            ))
        }
        "error" => {
            let agent_type = payload
                .pointer("/data/agent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown Agent");
            let message = payload
                .pointer("/data/message")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            Some((
                "error".to_string(),
                message_formatter::format_agent_error(agent_type, message),
            ))
        }
        "status_changed" => {
            let status = payload
                .pointer("/data/status")
                .and_then(|v| v.as_str())?;
            if status != "disconnected" {
                return None;
            }
            let agent_type = payload
                .pointer("/data/agent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown Agent");
            Some((
                "status_disconnected".to_string(),
                message_formatter::format_agent_disconnected(agent_type),
            ))
        }
        // Phase 2: "permission_request" will be handled here
        _ => None,
    }
}

fn parse_git_push(payload: &serde_json::Value) -> Option<(String, RichMessage)> {
    let folder_name = payload
        .get("folder_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let branch = payload
        .get("branch")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let commits = payload
        .get("pushed_commits")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    Some((
        "git_push".to_string(),
        message_formatter::format_git_push(folder_name, branch, commits),
    ))
}

fn parse_git_commit(payload: &serde_json::Value) -> Option<(String, RichMessage)> {
    let folder_name = payload
        .get("folder_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let files = payload
        .get("committed_files")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    Some((
        "git_commit".to_string(),
        message_formatter::format_git_commit(folder_name, files),
    ))
}
