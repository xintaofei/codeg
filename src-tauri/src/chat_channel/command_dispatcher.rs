use std::sync::Arc;
use std::time::{Duration, Instant};

use sea_orm::DatabaseConnection;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

use super::authz;
use super::command_handlers;
use super::i18n::{self, Lang};
use super::manager::ChatChannelManager;
use super::session_bridge::SessionBridge;
use super::session_commands;
use super::types::IncomingCommand;
use crate::acp::manager::ConnectionManager;
use crate::db::service::{
    app_metadata_service, chat_channel_message_log_service, chat_channel_service,
};
use crate::web::event_bridge::EventEmitter;

const COMMAND_PREFIX_KEY: &str = "chat_command_prefix";
const DEFAULT_COMMAND_PREFIX: &str = "/";
const MESSAGE_LANGUAGE_KEY: &str = "chat_message_language";
/// How often to refresh cached config from DB.
const CONFIG_CACHE_TTL_SECS: u64 = 30;

struct CommandConfigCache {
    prefix: String,
    lang: Lang,
    last_refresh: Instant,
}

impl CommandConfigCache {
    fn new() -> Self {
        Self {
            prefix: DEFAULT_COMMAND_PREFIX.to_string(),
            lang: Lang::default(),
            // Force refresh on first use
            last_refresh: Instant::now() - Duration::from_secs(CONFIG_CACHE_TTL_SECS + 1),
        }
    }

    async fn refresh_if_needed(&mut self, db: &DatabaseConnection) {
        if self.last_refresh.elapsed() < Duration::from_secs(CONFIG_CACHE_TTL_SECS) {
            return;
        }

        if let Ok(Some(val)) = app_metadata_service::get_value(db, COMMAND_PREFIX_KEY).await {
            self.prefix = val;
        }
        if let Ok(Some(val)) = app_metadata_service::get_value(db, MESSAGE_LANGUAGE_KEY).await {
            self.lang = Lang::from_str_lossy(&val);
        }

        self.last_refresh = Instant::now();
    }
}

pub fn spawn_command_dispatcher(
    mut command_rx: mpsc::Receiver<IncomingCommand>,
    manager: ChatChannelManager,
    db_conn: DatabaseConnection,
    conn_mgr: ConnectionManager,
    emitter: EventEmitter,
    bridge: Arc<Mutex<SessionBridge>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut config = CommandConfigCache::new();

        while let Some(cmd) = command_rx.recv().await {
            let text = cmd.command_text.trim();
            tracing::info!(
                "[ChatChannel] received command from channel={} sender={}: {:?}",
                cmd.channel_id,
                cmd.sender_id,
                text
            );

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

            config.refresh_if_needed(&db_conn).await;

            let response = dispatch_command(
                text,
                &config.prefix,
                &db_conn,
                &manager,
                &conn_mgr,
                &emitter,
                &bridge,
                cmd.channel_id,
                &cmd.sender_id,
                config.lang,
            )
            .await;

            tracing::info!(
                "[ChatChannel] dispatch result: title={:?}, body_len={}",
                response.title,
                response.body.len()
            );

            // Send response back via the same channel
            let send_result = manager.send_to_channel(cmd.channel_id, &response).await;
            let (status, error_detail) = match &send_result {
                Ok(_) => ("sent", None),
                Err(e) => {
                    tracing::error!(
                        "[ChatChannel] failed to send response for {:?} to channel {}: {e}",
                        text,
                        cmd.channel_id
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

#[allow(clippy::too_many_arguments)]
async fn dispatch_command(
    text: &str,
    prefix: &str,
    db: &DatabaseConnection,
    manager: &ChatChannelManager,
    conn_mgr: &ConnectionManager,
    emitter: &EventEmitter,
    bridge: &Arc<Mutex<SessionBridge>>,
    channel_id: i32,
    sender_id: &str,
    lang: Lang,
) -> super::types::RichMessage {
    // ── Authorization gate (fail-closed) ──
    // Chat commands spawn and drive agents with the host's full privileges, so
    // every inbound message must come from an allow-listed sender. An empty or
    // unset allowlist authorizes no one. We reply with the sender's own id (and
    // run no command) so the operator can add it from the channel settings.
    let authorized = match chat_channel_service::get_by_id(db, channel_id).await {
        Ok(Some(ch)) => authz::is_sender_allowed(&ch.config_json, sender_id),
        Ok(None) => false,
        Err(e) => {
            tracing::error!(
                "[ChatChannel] authz lookup failed for channel={channel_id}: {e}; denying"
            );
            false
        }
    };
    if !authorized {
        tracing::warn!(
            "[ChatChannel] BLOCKED unauthorized sender={sender_id} on channel={channel_id}: {text:?}"
        );
        return super::types::RichMessage::error(i18n::unauthorized_sender_body(lang, sender_id))
            .with_title(i18n::unauthorized_sender_title(lang));
    }

    // Strip prefix; if text doesn't start with it, try as follow-up
    let without_prefix = match text.strip_prefix(prefix) {
        Some(rest) => rest,
        None => {
            // Check if sender has an active session for follow-up
            let has_session = {
                let guard = bridge.lock().await;
                guard.find_by_sender(channel_id, sender_id).is_some()
            };
            if has_session {
                return session_commands::handle_followup(session_commands::FollowupRequest {
                    db,
                    text,
                    channel_id,
                    sender_id,
                    conn_mgr,
                    bridge,
                    lang,
                    prefix,
                })
                .await;
            }
            return command_handlers::handle_help(prefix, lang);
        }
    };

    let parts: Vec<&str> = without_prefix.splitn(2, ' ').collect();
    let command = parts[0].to_lowercase();
    let args = parts.get(1).map(|s| s.trim()).unwrap_or("");

    match command.as_str() {
        // Existing commands
        "search" => {
            if args.is_empty() {
                super::types::RichMessage::info(i18n::search_usage(lang, prefix))
                    .with_title(i18n::invalid_args_title(lang))
            } else {
                command_handlers::handle_search(db, args, lang).await
            }
        }
        "today" => command_handlers::handle_today(db, lang).await,
        "status" => command_handlers::handle_status(manager, lang).await,
        "help" | "start" => command_handlers::handle_help(prefix, lang),

        // Session commands
        "folder" => {
            session_commands::handle_folder(db, args, channel_id, sender_id, lang, prefix).await
        }
        "agent" => {
            session_commands::handle_agent(db, args, channel_id, sender_id, lang, prefix).await
        }
        "task" | "do" => {
            session_commands::handle_task(
                db, args, channel_id, sender_id, conn_mgr, emitter, bridge, lang, prefix,
            )
            .await
        }
        "sessions" => {
            session_commands::handle_sessions(db, channel_id, sender_id, lang, prefix).await
        }
        "resume" => {
            session_commands::handle_resume(
                db, args, channel_id, sender_id, conn_mgr, emitter, bridge, lang, prefix,
            )
            .await
        }
        "cancel" => {
            session_commands::handle_cancel(db, channel_id, sender_id, conn_mgr, bridge, lang).await
        }
        "approve" => {
            let always = args.eq_ignore_ascii_case("always");
            session_commands::handle_permission_response(
                true, always, db, channel_id, sender_id, conn_mgr, bridge, lang,
            )
            .await
        }
        "deny" => {
            session_commands::handle_permission_response(
                false, false, db, channel_id, sender_id, conn_mgr, bridge, lang,
            )
            .await
        }

        _ => super::types::RichMessage::info(i18n::unknown_command(lang, prefix, &command))
            .with_title(i18n::unknown_command_title(lang)),
    }
}
