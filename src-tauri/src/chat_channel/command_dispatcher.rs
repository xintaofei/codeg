use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::command_handlers;
use super::manager::ChatChannelManager;
use super::types::IncomingCommand;
use crate::db::service::{app_metadata_service, chat_channel_message_log_service};

const COMMAND_PREFIX_KEY: &str = "chat_command_prefix";
const DEFAULT_COMMAND_PREFIX: &str = "/";

pub fn spawn_command_dispatcher(
    mut command_rx: mpsc::Receiver<IncomingCommand>,
    manager: ChatChannelManager,
    db_conn: DatabaseConnection,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(cmd) = command_rx.recv().await {
            let text = cmd.command_text.trim();

            // Log inbound command
            let _ = chat_channel_message_log_service::create_log(
                &db_conn,
                cmd.channel_id,
                "inbound",
                "command_query",
                text,
                "sent",
                None,
            )
            .await;

            let prefix = app_metadata_service::get_value(&db_conn, COMMAND_PREFIX_KEY)
                .await
                .ok()
                .flatten()
                .unwrap_or_else(|| DEFAULT_COMMAND_PREFIX.to_string());

            let response = dispatch_command(text, &prefix, &db_conn, &manager).await;

            // Send response back via the same channel
            let send_result = manager.send_to_channel(cmd.channel_id, &response).await;
            let (status, error_detail) = match &send_result {
                Ok(_) => ("sent", None),
                Err(e) => {
                    eprintln!(
                        "[ChatChannel] failed to send response for {:?} to channel {}: {e}",
                        text, cmd.channel_id
                    );
                    ("failed", Some(e.to_string()))
                }
            };

            let _ = chat_channel_message_log_service::create_log(
                &db_conn,
                cmd.channel_id,
                "outbound",
                "command_response",
                &response.to_plain_text(),
                status,
                error_detail,
            )
            .await;
        }
    })
}

async fn dispatch_command(
    text: &str,
    prefix: &str,
    db: &DatabaseConnection,
    manager: &ChatChannelManager,
) -> super::types::RichMessage {
    // Check if text starts with the configured prefix
    if !text.starts_with(prefix) {
        return command_handlers::handle_help(prefix);
    }

    // Strip prefix and parse command + args
    let without_prefix = &text[prefix.len()..];
    let parts: Vec<&str> = without_prefix.splitn(2, ' ').collect();
    let command = parts[0].to_lowercase();
    let args = parts.get(1).map(|s| s.trim()).unwrap_or("");

    match command.as_str() {
        "recent" => command_handlers::handle_recent(db).await,
        "search" => {
            if args.is_empty() {
                super::types::RichMessage::info(format!("用法: {prefix}search <关键词>"))
                    .with_title("参数错误")
            } else {
                command_handlers::handle_search(db, args).await
            }
        }
        "detail" => {
            if let Ok(id) = args.parse::<i32>() {
                command_handlers::handle_detail(db, id).await
            } else {
                super::types::RichMessage::info(format!("用法: {prefix}detail <会话ID>"))
                    .with_title("参数错误")
            }
        }
        "today" => command_handlers::handle_today(db).await,
        "status" => command_handlers::handle_status(manager).await,
        "help" | "start" => command_handlers::handle_help(prefix),
        _ => {
            super::types::RichMessage::info(format!(
                "未知命令: {prefix}{command}\n输入 {prefix}help 查看可用命令",
            ))
            .with_title("未知命令")
        }
    }
}
