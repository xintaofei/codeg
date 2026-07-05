use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use tokio::sync::Mutex;

use super::i18n::{self, Lang};
use super::manager::ChatChannelManager;
use super::session_bridge::{ActiveSession, SessionBridge};
use super::types::{
    ButtonStyle, ChannelMessageTarget, InteractiveMessage, MessageButton, MessageLevel, RichMessage,
};
use crate::acp::manager::ConnectionManager;
use crate::acp::registry::all_acp_agents;
use crate::acp::types::PromptInputBlock;
use crate::db::entities::conversation;
use crate::db::service::{
    conversation_service, folder_service, sender_context_service, thread_binding_service,
};
use crate::models::agent::AgentType;
use crate::web::event_bridge::EventEmitter;

pub struct FollowupRequest<'a> {
    pub db: &'a DatabaseConnection,
    pub text: &'a str,
    pub channel_id: i32,
    pub sender_id: &'a str,
    pub target: &'a ChannelMessageTarget,
    pub conn_mgr: &'a ConnectionManager,
    pub bridge: &'a Arc<Mutex<SessionBridge>>,
    pub lang: Lang,
    pub prefix: &'a str,
}

pub struct CommandMessageResult {
    pub message: RichMessage,
    pub response_target: ChannelMessageTarget,
}

impl CommandMessageResult {
    fn current_target(message: RichMessage, target: &ChannelMessageTarget) -> Self {
        Self {
            message,
            response_target: target.clone(),
        }
    }
}

pub enum SessionCommandMessage {
    Rich(RichMessage),
    Interactive(InteractiveMessage),
}

impl From<RichMessage> for SessionCommandMessage {
    fn from(message: RichMessage) -> Self {
        Self::Rich(message)
    }
}

struct CommandSessionRef {
    connection_id: String,
    conversation_id: Option<i32>,
    binding_id: Option<i32>,
}

// ── /folder ──

pub async fn handle_folder(
    db: &DatabaseConnection,
    args: &str,
    channel_id: i32,
    sender_id: &str,
    lang: Lang,
    prefix: &str,
) -> RichMessage {
    if args.is_empty() {
        return list_folders(db, channel_id, sender_id, lang, prefix).await;
    }

    // Try parse as index (1-based)
    if let Ok(idx) = args.parse::<usize>() {
        return select_folder_by_index(db, idx, channel_id, sender_id, lang, prefix).await;
    }

    // Treat as path
    select_folder_by_path(db, args, channel_id, sender_id, lang).await
}

pub async fn handle_folder_picker(
    db: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    lang: Lang,
    prefix: &str,
) -> SessionCommandMessage {
    let folders = match folder_service::list_folders(db).await {
        Ok(f) => f,
        Err(e) => {
            return RichMessage::error(format!("{}{e}", i18n::failed_to_list_folders_label(lang)))
                .into();
        }
    };

    if folders.is_empty() {
        return RichMessage::info(i18n::no_folders_found(lang))
            .with_title(i18n::folder_title(lang))
            .into();
    }

    let ctx = sender_context_service::get_or_create(db, channel_id, sender_id)
        .await
        .ok();

    let mut body = String::new();
    let mut buttons = Vec::new();
    for (i, f) in folders.iter().take(10).enumerate() {
        let current = ctx
            .as_ref()
            .and_then(|c| c.current_folder_id)
            .map(|id| id == f.id)
            .unwrap_or(false);
        let marker = if current { " [*]" } else { "" };
        body.push_str(&format!("{}. {}{} ({})\n", i + 1, f.name, marker, f.path));
        buttons.push(MessageButton {
            id: format!("cfg:folder:{}", f.id),
            label: truncate_button_label(&format!("{}{}", i + 1, f.name), 40),
            style: ButtonStyle::Default,
        });
    }

    body.push_str(&format!("\n{}", i18n::folder_select_hint(lang, prefix)));

    SessionCommandMessage::Interactive(InteractiveMessage {
        base: RichMessage::info(body.trim_end()).with_title(i18n::folder_title(lang)),
        buttons,
        callback_context: serde_json::json!({ "kind": "folder" }),
    })
}

async fn list_folders(
    db: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    lang: Lang,
    prefix: &str,
) -> RichMessage {
    let folders = match folder_service::list_folders(db).await {
        Ok(f) => f,
        Err(e) => {
            return RichMessage::error(format!("{}{e}", i18n::failed_to_list_folders_label(lang)));
        }
    };

    if folders.is_empty() {
        return RichMessage::info(i18n::no_folders_found(lang))
            .with_title(i18n::folder_title(lang));
    }

    let ctx = sender_context_service::get_or_create(db, channel_id, sender_id)
        .await
        .ok();

    let mut body = String::new();
    for (i, f) in folders.iter().take(10).enumerate() {
        let current = ctx
            .as_ref()
            .and_then(|c| c.current_folder_id)
            .map(|id| id == f.id)
            .unwrap_or(false);
        let marker = if current { " [*]" } else { "" };
        body.push_str(&format!("{}. {}{} ({})\n", i + 1, f.name, marker, f.path));
    }

    body.push_str(&format!("\n{}", i18n::folder_select_hint(lang, prefix)));

    RichMessage::info(body.trim_end()).with_title(i18n::folder_title(lang))
}

async fn select_folder_by_index(
    db: &DatabaseConnection,
    idx: usize,
    channel_id: i32,
    sender_id: &str,
    lang: Lang,
    prefix: &str,
) -> RichMessage {
    if idx == 0 {
        return RichMessage::info(i18n::index_starts_from_one(lang));
    }

    let folders = match folder_service::list_folders(db).await {
        Ok(f) => f,
        Err(e) => {
            return RichMessage::error(format!("{}{e}", i18n::failed_to_list_folders_label(lang)));
        }
    };

    let Some(folder) = folders.get(idx - 1) else {
        return RichMessage::info(i18n::folder_index_out_of_range(lang, prefix));
    };

    let _ = sender_context_service::update_folder(db, channel_id, sender_id, Some(folder.id)).await;

    RichMessage::info(format!("{} ({})", folder.name, folder.path))
        .with_title(i18n::folder_selected_title(lang))
}

async fn select_folder_by_id(
    db: &DatabaseConnection,
    folder_id: i32,
    channel_id: i32,
    sender_id: &str,
    lang: Lang,
) -> RichMessage {
    let folder = match folder_service::get_folder_by_id(db, folder_id).await {
        Ok(Some(folder)) => folder,
        _ => {
            return RichMessage::info(i18n::folder_not_found(lang));
        }
    };

    let _ = sender_context_service::update_folder(db, channel_id, sender_id, Some(folder.id)).await;

    RichMessage::info(format!("{} ({})", folder.name, folder.path))
        .with_title(i18n::folder_selected_title(lang))
}

async fn select_folder_by_path(
    db: &DatabaseConnection,
    path: &str,
    channel_id: i32,
    sender_id: &str,
    lang: Lang,
) -> RichMessage {
    let entry = match folder_service::add_folder(db, path).await {
        Ok(e) => e,
        Err(e) => {
            return RichMessage::error(format!("{}{e}", i18n::failed_to_add_folder_label(lang)));
        }
    };

    let _ = sender_context_service::update_folder(db, channel_id, sender_id, Some(entry.id)).await;

    RichMessage::info(format!("{} ({})", entry.name, entry.path))
        .with_title(i18n::folder_selected_title(lang))
}

// ── /agent ──

pub async fn handle_agent(
    db: &DatabaseConnection,
    args: &str,
    channel_id: i32,
    sender_id: &str,
    lang: Lang,
    prefix: &str,
) -> RichMessage {
    if args.is_empty() {
        return list_agents(db, channel_id, sender_id, lang, prefix).await;
    }

    // Try parse as index
    if let Ok(idx) = args.parse::<usize>() {
        return select_agent_by_index(db, idx, channel_id, sender_id, lang, prefix).await;
    }

    // Try parse as agent type name
    select_agent_by_name(db, args, channel_id, sender_id, lang).await
}

pub async fn handle_agent_picker(
    db: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    lang: Lang,
    prefix: &str,
) -> SessionCommandMessage {
    let agents = all_acp_agents();
    let ctx = sender_context_service::get_or_create(db, channel_id, sender_id)
        .await
        .ok();

    let mut body = String::new();
    let mut buttons = Vec::new();
    for (i, at) in agents.iter().enumerate() {
        let at_str = agent_type_to_string(*at);
        let current = ctx
            .as_ref()
            .and_then(|c| c.current_agent_type.as_deref())
            .map(|s| s == at_str)
            .unwrap_or(false);
        let marker = if current { " [*]" } else { "" };
        body.push_str(&format!("{}. {}{}\n", i + 1, at, marker));
        buttons.push(MessageButton {
            id: format!("cfg:agent:{at_str}"),
            label: truncate_button_label(&at.to_string(), 40),
            style: ButtonStyle::Default,
        });
    }

    body.push_str(&format!("\n{}", i18n::agent_select_hint(lang, prefix)));

    SessionCommandMessage::Interactive(InteractiveMessage {
        base: RichMessage::info(body.trim_end()).with_title(i18n::agent_title(lang)),
        buttons,
        callback_context: serde_json::json!({ "kind": "agent" }),
    })
}

async fn list_agents(
    db: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    lang: Lang,
    prefix: &str,
) -> RichMessage {
    let agents = all_acp_agents();
    let ctx = sender_context_service::get_or_create(db, channel_id, sender_id)
        .await
        .ok();

    let mut body = String::new();
    for (i, at) in agents.iter().enumerate() {
        let at_str = agent_type_to_string(*at);
        let current = ctx
            .as_ref()
            .and_then(|c| c.current_agent_type.as_deref())
            .map(|s| s == at_str)
            .unwrap_or(false);
        let marker = if current { " [*]" } else { "" };
        body.push_str(&format!("{}. {}{}\n", i + 1, at, marker));
    }

    body.push_str(&format!("\n{}", i18n::agent_select_hint(lang, prefix)));

    RichMessage::info(body.trim_end()).with_title(i18n::agent_title(lang))
}

async fn select_agent_by_index(
    db: &DatabaseConnection,
    idx: usize,
    channel_id: i32,
    sender_id: &str,
    lang: Lang,
    prefix: &str,
) -> RichMessage {
    let agents = all_acp_agents();
    if idx == 0 || idx > agents.len() {
        return RichMessage::info(i18n::agent_index_out_of_range(lang, prefix));
    }

    let at = agents[idx - 1];
    let at_str = agent_type_to_string(at);
    let _ = sender_context_service::update_agent(db, channel_id, sender_id, Some(at_str)).await;

    RichMessage::info(at.to_string()).with_title(i18n::agent_selected_title(lang))
}

async fn select_agent_by_name(
    db: &DatabaseConnection,
    name: &str,
    channel_id: i32,
    sender_id: &str,
    lang: Lang,
) -> RichMessage {
    let at = match parse_agent_type(name) {
        Some(a) => a,
        None => {
            return RichMessage::info(format!("{}{}", i18n::unknown_agent_label(lang), name));
        }
    };

    let at_str = agent_type_to_string(at);
    let _ = sender_context_service::update_agent(db, channel_id, sender_id, Some(at_str)).await;

    RichMessage::info(at.to_string()).with_title(i18n::agent_selected_title(lang))
}

pub async fn handle_callback(
    db: &DatabaseConnection,
    data: &str,
    channel_id: i32,
    sender_id: &str,
    lang: Lang,
    prefix: &str,
) -> RichMessage {
    if let Some(folder_id) = data.strip_prefix("cfg:folder:") {
        let Ok(folder_id) = folder_id.parse::<i32>() else {
            return RichMessage::info(callback_expired_or_invalid(lang, prefix));
        };
        return select_folder_by_id(db, folder_id, channel_id, sender_id, lang).await;
    }

    if let Some(agent) = data.strip_prefix("cfg:agent:") {
        return select_agent_by_name(db, agent, channel_id, sender_id, lang).await;
    }

    RichMessage::info(callback_expired_or_invalid(lang, prefix))
}

// ── /task ──

#[allow(clippy::too_many_arguments)]
pub async fn handle_task(
    db: &DatabaseConnection,
    task_description: &str,
    channel_id: i32,
    sender_id: &str,
    target: &ChannelMessageTarget,
    manager: &ChatChannelManager,
    conn_mgr: &ConnectionManager,
    emitter: &EventEmitter,
    bridge: &Arc<Mutex<SessionBridge>>,
    lang: Lang,
    prefix: &str,
) -> CommandMessageResult {
    if task_description.is_empty() {
        return CommandMessageResult::current_target(
            RichMessage::info(i18n::task_usage(lang, prefix)),
            target,
        );
    }

    if has_active_topic_session(db, bridge, target).await {
        return CommandMessageResult::current_target(
            RichMessage::info(topic_has_active_session(lang, prefix)),
            target,
        );
    }

    // 1. Load sender context
    let ctx = match sender_context_service::get_or_create(db, channel_id, sender_id).await {
        Ok(c) => c,
        Err(e) => {
            return CommandMessageResult::current_target(
                RichMessage::error(format!("{}{e}", i18n::failed_to_load_context_label(lang))),
                target,
            );
        }
    };

    let folder_id = match ctx.current_folder_id {
        Some(id) => id,
        None => {
            return CommandMessageResult::current_target(
                RichMessage::info(i18n::no_folder_selected(lang, prefix)),
                target,
            );
        }
    };

    // 2. Get folder info
    let folder = match folder_service::get_folder_by_id(db, folder_id).await {
        Ok(Some(f)) => f,
        _ => {
            return CommandMessageResult::current_target(
                RichMessage::info(i18n::folder_not_found_with_hint(lang, prefix)),
                target,
            );
        }
    };

    // 3. Resolve agent type
    let agent_type = match resolve_agent_type(&ctx.current_agent_type, &folder.default_agent_type) {
        Some(at) => at,
        None => {
            return CommandMessageResult::current_target(
                RichMessage::info(i18n::no_agent_selected(lang, prefix)),
                target,
            );
        }
    };

    let mut session_target = target.clone();
    if target.is_telegram_general_topic() {
        match manager
            .create_thread(channel_id, &truncate_topic_title(task_description))
            .await
        {
            Ok(created) => {
                session_target = created;
            }
            Err(e) => {
                return CommandMessageResult::current_target(
                    RichMessage::error(topic_create_failed(lang, &e.to_string())),
                    target,
                );
            }
        }
    }

    // 4. Create conversation record
    let conv = match conversation_service::create(
        db,
        folder_id,
        agent_type,
        Some(truncate_title(task_description)),
        folder.git_branch.clone(),
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            return CommandMessageResult::current_target(
                RichMessage::error(format!(
                    "{}{e}",
                    i18n::failed_to_create_conversation_label(lang)
                )),
                target,
            );
        }
    };

    // 5. Spawn ACP agent
    let owner_label = owner_label_for(channel_id, sender_id, &session_target);
    let connection_id = match conn_mgr
        .spawn_agent(
            agent_type,
            Some(folder.path.clone()),
            None,
            BTreeMap::new(),
            owner_label,
            emitter.clone(),
            None,
            BTreeMap::new(),
        )
        .await
    {
        Ok(id) => id,
        Err(e) => {
            // Clean up the conversation record
            let _ = conversation_service::update_status(
                db,
                conv.id,
                conversation::ConversationStatus::Cancelled,
            )
            .await;
            return CommandMessageResult::current_target(
                RichMessage::error(format!("{}{e}", i18n::failed_to_start_agent_label(lang))),
                target,
            );
        }
    };

    if session_target.is_telegram_forum_topic() {
        if let Err(e) = thread_binding_service::upsert_for_target(
            db,
            &session_target,
            "telegram",
            conv.id,
            Some(connection_id.clone()),
            sender_id,
            conv.title.clone(),
        )
        .await
        {
            let _ = conn_mgr.cancel(db, &connection_id).await;
            let _ = conversation_service::update_status(
                db,
                conv.id,
                conversation::ConversationStatus::Cancelled,
            )
            .await;
            return CommandMessageResult::current_target(
                RichMessage::error(format!("Failed to bind topic: {e}")),
                target,
            );
        }
        if let Some(title) = conv.title.as_deref() {
            manager.sync_conversation_title(db, conv.id, title).await;
        }
    }

    // 6. Register in bridge (prompt will be sent after SessionStarted event)
    {
        let session = ActiveSession {
            channel_id,
            sender_id: sender_id.to_string(),
            target: session_target.clone(),
            conversation_id: conv.id,
            connection_id: connection_id.clone(),
            agent_type,
            content_buffer: String::new(),
            tool_calls: Vec::new(),
            tool_call_inputs: std::collections::HashMap::new(),
            delegation_rendered: std::collections::HashSet::new(),
            last_flushed: Instant::now(),
            pending_prompt: Some(task_description.to_string()),
            permission_pending: None,
        };
        bridge.lock().await.register(connection_id.clone(), session);
    }

    // 7. Update sender context only for legacy non-topic routing.
    if !session_target.is_telegram_forum_topic() {
        let _ = sender_context_service::update_session(
            db,
            channel_id,
            sender_id,
            Some(conv.id),
            Some(connection_id),
        )
        .await;
    }

    CommandMessageResult {
        message: RichMessage::info(format!("[{}] #{} @ {}", agent_type, conv.id, folder.name,))
            .with_title(i18n::task_started_title(lang)),
        response_target: session_target,
    }
}

// ── /sessions ──

pub async fn handle_sessions(
    db: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    target: &ChannelMessageTarget,
    lang: Lang,
    prefix: &str,
) -> RichMessage {
    let ctx = match sender_context_service::get_or_create(db, channel_id, sender_id).await {
        Ok(c) => c,
        Err(e) => {
            return RichMessage::error(format!("{}{e}", i18n::failed_to_load_context_label(lang)));
        }
    };

    let topic_conversation_id = if target.is_telegram_forum_topic() {
        thread_binding_service::get_by_target(db, target)
            .await
            .ok()
            .flatten()
            .map(|b| b.conversation_id)
    } else {
        None
    };
    let current_conversation_id =
        if target.is_telegram_forum_topic() || target.is_telegram_general_topic() {
            topic_conversation_id
        } else {
            ctx.current_conversation_id
        };

    let folder_id = match ctx.current_folder_id {
        Some(id) => id,
        None => {
            return RichMessage::info(i18n::no_folder_selected(lang, prefix));
        }
    };

    let folder = match folder_service::get_folder_by_id(db, folder_id).await {
        Ok(Some(f)) => f,
        _ => {
            return RichMessage::info(i18n::folder_not_found(lang));
        }
    };

    let convs = match conversation_service::list_by_folder(
        db,
        folder_id,
        None,
        None,
        None,
        Some("in_progress".to_string()),
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            return RichMessage::error(format!("{}{e}", i18n::failed_to_list_sessions_label(lang)));
        }
    };

    if convs.is_empty() {
        return RichMessage::info(i18n::no_active_sessions_in_folder(lang)).with_title(format!(
            "{} - {}",
            i18n::sessions_title(lang),
            folder.name
        ));
    }

    let mut body = String::new();
    for (i, c) in convs.iter().take(10).enumerate() {
        let title = c.title.as_deref().unwrap_or("(untitled)");
        let current = current_conversation_id
            .map(|id| id == c.id)
            .unwrap_or(false);
        let marker = if current { " [*]" } else { "" };
        body.push_str(&format!(
            "{}. [{}] {} (#{}){}  \n",
            i + 1,
            c.agent_type,
            title,
            c.id,
            marker,
        ));
    }

    body.push_str(&format!("\n{}", i18n::sessions_resume_hint(lang, prefix)));

    RichMessage::info(body.trim_end()).with_title(format!(
        "{} - {}",
        i18n::sessions_title(lang),
        folder.name
    ))
}

// ── /resume ──

#[allow(clippy::too_many_arguments)]
pub async fn handle_resume(
    db: &DatabaseConnection,
    args: &str,
    channel_id: i32,
    sender_id: &str,
    target: &ChannelMessageTarget,
    manager: &ChatChannelManager,
    conn_mgr: &ConnectionManager,
    emitter: &EventEmitter,
    bridge: &Arc<Mutex<SessionBridge>>,
    lang: Lang,
    prefix: &str,
) -> RichMessage {
    if args.is_empty() {
        return list_recent_sessions(db, lang, prefix).await;
    }

    let conversation_id: i32 = match args.parse() {
        Ok(id) => id,
        Err(_) => {
            return list_recent_sessions(db, lang, prefix).await;
        }
    };

    if target.is_telegram_general_topic() {
        return RichMessage::info(no_topic_session_use_task_or_resume(lang, prefix));
    }

    let conv = match conversation_service::get_by_id(db, conversation_id).await {
        Ok(c) => c,
        Err(_) => {
            return RichMessage::info(i18n::conversation_not_found(lang));
        }
    };

    if has_active_topic_session(db, bridge, target).await {
        return RichMessage::info(topic_has_active_session(lang, prefix));
    }

    let folder = match folder_service::get_folder_by_id(db, conv.folder_id).await {
        Ok(Some(f)) => f,
        _ => {
            return RichMessage::info(i18n::folder_not_found(lang));
        }
    };

    // Spawn agent with session_id for resume
    let owner_label = owner_label_for(channel_id, sender_id, target);
    let connection_id = match conn_mgr
        .spawn_agent(
            conv.agent_type,
            Some(folder.path.clone()),
            conv.external_id.clone(),
            BTreeMap::new(),
            owner_label,
            emitter.clone(),
            None,
            BTreeMap::new(),
        )
        .await
    {
        Ok(id) => id,
        Err(e) => {
            return RichMessage::error(format!("{}{e}", i18n::failed_to_start_agent_label(lang)));
        }
    };

    // Register in bridge (no pending prompt for resume)
    {
        let session = ActiveSession {
            channel_id,
            sender_id: sender_id.to_string(),
            target: target.clone(),
            conversation_id: conv.id,
            connection_id: connection_id.clone(),
            agent_type: conv.agent_type,
            content_buffer: String::new(),
            tool_calls: Vec::new(),
            tool_call_inputs: std::collections::HashMap::new(),
            delegation_rendered: std::collections::HashSet::new(),
            last_flushed: Instant::now(),
            pending_prompt: None,
            permission_pending: None,
        };
        bridge.lock().await.register(connection_id.clone(), session);
    }

    if target.is_telegram_forum_topic() {
        if let Err(e) = thread_binding_service::upsert_for_target(
            db,
            target,
            "telegram",
            conv.id,
            Some(connection_id.clone()),
            sender_id,
            conv.title.clone(),
        )
        .await
        {
            let _ = conn_mgr.cancel(db, &connection_id).await;
            bridge.lock().await.remove(&connection_id);
            return RichMessage::error(format!("Failed to bind topic: {e}"));
        }
        if let Some(title) = conv.title.as_deref() {
            manager.sync_conversation_title(db, conv.id, title).await;
        }
    }

    // Update sender context only for legacy non-topic routing.
    if !target.is_telegram_forum_topic() {
        let _ = sender_context_service::update_session(
            db,
            channel_id,
            sender_id,
            Some(conv.id),
            Some(connection_id),
        )
        .await;
    }
    let _ = sender_context_service::update_folder(db, channel_id, sender_id, Some(conv.folder_id))
        .await;

    let title = conv.title.as_deref().unwrap_or("(untitled)");
    RichMessage::info(format!(
        "[{}] #{} {} @ {}",
        conv.agent_type, conv.id, title, folder.name,
    ))
    .with_title(i18n::session_resumed_title(lang))
}

// ── /cancel ──

pub async fn handle_cancel(
    db: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    target: &ChannelMessageTarget,
    conn_mgr: &ConnectionManager,
    bridge: &Arc<Mutex<SessionBridge>>,
    lang: Lang,
) -> RichMessage {
    let session_ref = match command_session_ref(db, bridge, channel_id, sender_id, target).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return RichMessage::info(i18n::no_active_session_to_cancel(lang));
        }
        Err(e) => {
            return RichMessage::error(format!("{}{e}", i18n::failed_to_load_context_label(lang)));
        }
    };

    // Cancel the ACP connection (also CAS-updates the row to Cancelled and
    // emits ConversationStatusChanged when the row is still InProgress).
    let _ = conn_mgr.cancel(db, &session_ref.connection_id).await;

    // Remove from bridge
    bridge.lock().await.remove(&session_ref.connection_id);

    // Update conversation status
    if let Some(conv_id) = session_ref.conversation_id {
        let _ = conversation_service::update_status(
            db,
            conv_id,
            conversation::ConversationStatus::Cancelled,
        )
        .await;
    }

    // Clear session from context
    if let Some(binding_id) = session_ref.binding_id {
        let _ = thread_binding_service::clear_connection(db, binding_id).await;
    } else {
        let _ = sender_context_service::clear_session(db, channel_id, sender_id).await;
    }

    RichMessage::info(i18n::task_cancelled_body(lang)).with_title(i18n::task_cancelled_title(lang))
}

// ── /approve, /deny ──

#[allow(clippy::too_many_arguments)]
pub async fn handle_permission_response(
    approve: bool,
    always: bool,
    db: &DatabaseConnection,
    channel_id: i32,
    sender_id: &str,
    target: &ChannelMessageTarget,
    conn_mgr: &ConnectionManager,
    bridge: &Arc<Mutex<SessionBridge>>,
    lang: Lang,
) -> RichMessage {
    let session_ref = match command_session_ref(db, bridge, channel_id, sender_id, target).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return RichMessage::info(i18n::no_active_session(lang));
        }
        Err(e) => {
            return RichMessage::error(format!("{}{e}", i18n::failed_to_load_context_label(lang)));
        }
    };

    let pending = {
        let mut bridge_guard = bridge.lock().await;
        let session = match bridge_guard.get_mut(&session_ref.connection_id) {
            Some(s) => s,
            None => {
                if let Some(binding_id) = session_ref.binding_id {
                    let _ = thread_binding_service::clear_connection(db, binding_id).await;
                }
                return RichMessage::info(i18n::no_active_session_found(lang));
            }
        };
        session.permission_pending.take()
    };

    let pending = match pending {
        Some(p) => p,
        None => {
            return RichMessage::info(i18n::no_pending_permission(lang));
        }
    };

    // Find the appropriate option_id
    let option_id = if approve {
        pending
            .options
            .iter()
            .find(|o| o.kind == "allow" || o.kind == "allowForSession")
            .or_else(|| pending.options.first())
            .map(|o| o.option_id.clone())
    } else {
        pending
            .options
            .iter()
            .find(|o| o.kind == "deny")
            .or_else(|| pending.options.last())
            .map(|o| o.option_id.clone())
    };

    let Some(option_id) = option_id else {
        return RichMessage::info(i18n::no_valid_permission_option(lang));
    };

    if let Err(e) = conn_mgr
        .respond_permission(&session_ref.connection_id, &pending.request_id, &option_id)
        .await
    {
        return RichMessage::error(format!(
            "{}{e}",
            i18n::failed_permission_response_label(lang)
        ));
    }

    // Update auto_approve if requested
    if always && approve {
        let _ = sender_context_service::update_auto_approve(db, channel_id, sender_id, true).await;
    }

    let action = if approve {
        i18n::approved_label(lang)
    } else {
        i18n::denied_label(lang)
    };

    let mut msg = RichMessage::info(format!("{}: {}", action, pending.tool_description));
    if always && approve {
        msg = msg.with_field("", i18n::auto_approve_enabled(lang));
    }
    msg.with_title(i18n::permission_response_title(lang))
}

// ── follow-up (non-command text) ──

pub async fn handle_followup(req: FollowupRequest<'_>) -> RichMessage {
    let session_ref = match command_session_ref(
        req.db,
        req.bridge,
        req.channel_id,
        req.sender_id,
        req.target,
    )
    .await
    {
        Ok(Some(s)) => s,
        Ok(None) => {
            let body = if req.target.is_telegram_forum_topic() {
                no_topic_session_use_task_or_resume(req.lang, req.prefix)
            } else {
                i18n::no_active_session_use_task(req.lang, req.prefix)
            };
            return RichMessage::info(body);
        }
        Err(e) => {
            return RichMessage::error(format!(
                "{}{e}",
                i18n::failed_to_load_context_label(req.lang)
            ));
        }
    };

    let connection_id = session_ref.connection_id;

    // Check connection exists in bridge
    {
        let bridge_guard = req.bridge.lock().await;
        if bridge_guard.get(&connection_id).is_none() {
            // Connection lost, clear context
            drop(bridge_guard);
            if let Some(binding_id) = session_ref.binding_id {
                let _ = thread_binding_service::clear_connection(req.db, binding_id).await;
            } else {
                let _ =
                    sender_context_service::clear_session(req.db, req.channel_id, req.sender_id)
                        .await;
            }
            return RichMessage::info(i18n::session_connection_lost(req.lang, req.prefix));
        }
    }

    // Send prompt to agent
    let blocks = vec![PromptInputBlock::Text {
        text: req.text.to_string(),
    }];

    if let Err(e) = req.conn_mgr.send_prompt(&connection_id, blocks).await {
        // A turn is already in flight on this (shared) connection — another
        // client, or a previous prompt still running. This is transient: the
        // connection is alive, so do NOT tear down the bridge/session. Tell the
        // user to retry once the current turn finishes.
        if matches!(e, crate::acp::error::AcpError::TurnInProgress) {
            return RichMessage::info(i18n::agent_busy_retry(req.lang).to_string());
        }
        // Otherwise the connection may have died — clean up.
        req.bridge.lock().await.remove(&connection_id);
        if let Some(binding_id) = session_ref.binding_id {
            let _ = thread_binding_service::clear_connection(req.db, binding_id).await;
        } else {
            let _ =
                sender_context_service::clear_session(req.db, req.channel_id, req.sender_id).await;
        }
        return RichMessage::error(format!(
            "{}{e}",
            i18n::failed_to_send_message_label(req.lang)
        ));
    }

    RichMessage::info(i18n::message_sent(req.lang))
}

// ── /resume (list recent) ──

async fn list_recent_sessions(db: &DatabaseConnection, lang: Lang, prefix: &str) -> RichMessage {
    let recent = match conversation::Entity::find()
        .filter(conversation::Column::DeletedAt.is_null())
        .order_by_desc(conversation::Column::CreatedAt)
        .limit(10)
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return RichMessage {
                title: Some(i18n::query_failed_title(lang).to_string()),
                body: e.to_string(),
                fields: Vec::new(),
                level: MessageLevel::Error,
            };
        }
    };

    if recent.is_empty() {
        return RichMessage::info(i18n::no_conversations_found(lang))
            .with_title(i18n::recent_conversations_title(lang));
    }

    let mut body = String::new();
    for conv in &recent {
        let title = conv.title.as_deref().unwrap_or(i18n::untitled(lang));
        let agent = &conv.agent_type;
        let time = conv.created_at.format("%m-%d %H:%M");
        body.push_str(&format!("#{} [{}] {} ({})\n", conv.id, agent, title, time,));
    }

    body.push_str(&format!("\n{}", i18n::recent_resume_hint(lang, prefix)));

    RichMessage::info(body.trim_end()).with_title(i18n::recent_conversations_title(lang))
}

async fn has_active_topic_session(
    db: &DatabaseConnection,
    bridge: &Arc<Mutex<SessionBridge>>,
    target: &ChannelMessageTarget,
) -> bool {
    if !target.is_telegram_forum_topic() {
        return false;
    }

    let binding = thread_binding_service::get_by_target(db, target)
        .await
        .ok()
        .flatten();

    {
        let guard = bridge.lock().await;
        if guard.find_by_target(target).is_some() {
            return true;
        }
        if let Some(binding) = &binding {
            if let Some(connection_id) = binding.connection_id.as_deref() {
                if guard.get(connection_id).is_some() {
                    return true;
                }
            }
        }
    }

    false
}

async fn command_session_ref(
    db: &DatabaseConnection,
    bridge: &Arc<Mutex<SessionBridge>>,
    channel_id: i32,
    sender_id: &str,
    target: &ChannelMessageTarget,
) -> Result<Option<CommandSessionRef>, crate::db::error::DbError> {
    if target.is_telegram_forum_topic() {
        let binding = thread_binding_service::get_by_target(db, target).await?;
        let bridge_session = {
            let guard = bridge.lock().await;
            guard
                .find_by_target(target)
                .map(|session| CommandSessionRef {
                    connection_id: session.connection_id.clone(),
                    conversation_id: Some(session.conversation_id),
                    binding_id: binding.as_ref().map(|b| b.id),
                })
        };
        if bridge_session.is_some() {
            return Ok(bridge_session);
        }

        return Ok(binding.and_then(|b| {
            let conversation_id = b.conversation_id;
            let binding_id = b.id;
            b.connection_id.map(|connection_id| CommandSessionRef {
                connection_id,
                conversation_id: Some(conversation_id),
                binding_id: Some(binding_id),
            })
        }));
    }

    let ctx = sender_context_service::get_or_create(db, channel_id, sender_id).await?;
    Ok(ctx
        .current_connection_id
        .map(|connection_id| CommandSessionRef {
            connection_id,
            conversation_id: ctx.current_conversation_id,
            binding_id: None,
        }))
}

fn owner_label_for(channel_id: i32, sender_id: &str, target: &ChannelMessageTarget) -> String {
    if target.is_telegram_forum_topic() {
        let thread_key = target.thread_key.as_deref().unwrap_or_default();
        format!("chat_channel:{channel_id}:{sender_id}:thread:{thread_key}")
    } else {
        format!("chat_channel:{channel_id}:{sender_id}")
    }
}

fn truncate_topic_title(task_description: &str) -> String {
    let title = truncate_title(task_description);
    format!("Codeg: {title}").chars().take(128).collect()
}

fn topic_has_active_session(lang: Lang, prefix: &str) -> String {
    match lang {
        Lang::ZhCn | Lang::ZhTw => {
            format!("当前 topic 已有活跃会话。请继续发送 follow-up，或先使用 {prefix}cancel。")
        }
        _ => format!(
            "This topic already has an active session. Send a follow-up or use {prefix}cancel first."
        ),
    }
}

fn no_topic_session_use_task_or_resume(lang: Lang, prefix: &str) -> String {
    match lang {
        Lang::ZhCn | Lang::ZhTw => {
            format!("当前 topic 尚未绑定会话。使用 {prefix}task <描述> 开始，或 {prefix}resume <id> 恢复。")
        }
        _ => format!(
            "This topic is not bound to a session. Use {prefix}task <description> or {prefix}resume <id>."
        ),
    }
}

fn topic_create_failed(lang: Lang, detail: &str) -> String {
    match lang {
        Lang::ZhCn | Lang::ZhTw => format!(
            "创建 Telegram topic 失败：{detail}\n请确认当前 chat 是 forum supergroup，且 bot 拥有管理 topics 权限。"
        ),
        _ => format!(
            "Failed to create Telegram topic: {detail}\nMake sure this chat is a forum supergroup and the bot can manage topics."
        ),
    }
}

fn callback_expired_or_invalid(lang: Lang, prefix: &str) -> String {
    match lang {
        Lang::ZhCn | Lang::ZhTw => {
            format!("这个按钮已失效。请重新发送 {prefix}folder 或 {prefix}agent。")
        }
        _ => format!("This button is no longer valid. Send {prefix}folder or {prefix}agent again."),
    }
}

fn truncate_button_label(label: &str, max_chars: usize) -> String {
    if label.chars().count() <= max_chars {
        label.to_string()
    } else {
        let mut truncated: String = label.chars().take(max_chars.saturating_sub(3)).collect();
        truncated.push_str("...");
        truncated
    }
}

// ── Helpers ──

fn agent_type_to_string(at: AgentType) -> String {
    serde_json::to_value(at)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

fn parse_agent_type(name: &str) -> Option<AgentType> {
    let normalized = name.to_lowercase().replace([' ', '-'], "_");
    serde_json::from_value(serde_json::Value::String(normalized)).ok()
}

fn resolve_agent_type(
    sender_agent: &Option<String>,
    folder_default: &Option<AgentType>,
) -> Option<AgentType> {
    if let Some(ref at_str) = sender_agent {
        if let Some(at) = parse_agent_type(at_str) {
            return Some(at);
        }
    }
    folder_default.as_ref().copied()
}

fn truncate_title(s: &str) -> String {
    if s.chars().count() <= 80 {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(77).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::service::{chat_channel_service, sender_context_service};
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};

    async fn seed_chat_channel(db: &crate::db::AppDatabase) -> i32 {
        chat_channel_service::create(
            &db.conn,
            "Telegram test".to_string(),
            "telegram".to_string(),
            serde_json::json!({ "chat_id": "-100123", "topic_mode": true }).to_string(),
            true,
            false,
            None,
        )
        .await
        .expect("seed chat channel")
        .id
    }

    #[tokio::test]
    async fn callback_folder_selection_updates_sender_context() {
        let db = fresh_in_memory_db().await;
        let channel_id = seed_chat_channel(&db).await;
        let folder_id = seed_folder(&db, "/tmp/codeg-callback-folder").await;

        let message = handle_callback(
            &db.conn,
            &format!("cfg:folder:{folder_id}"),
            channel_id,
            "sender-1",
            Lang::En,
            "/",
        )
        .await;
        let ctx = sender_context_service::get_or_create(&db.conn, channel_id, "sender-1")
            .await
            .expect("context");

        assert_eq!(ctx.current_folder_id, Some(folder_id));
        assert_eq!(message.title.as_deref(), Some("Folder Selected"));
    }

    #[tokio::test]
    async fn callback_agent_selection_updates_sender_context() {
        let db = fresh_in_memory_db().await;
        let channel_id = seed_chat_channel(&db).await;

        let message = handle_callback(
            &db.conn,
            "cfg:agent:codex",
            channel_id,
            "sender-1",
            Lang::En,
            "/",
        )
        .await;
        let ctx = sender_context_service::get_or_create(&db.conn, channel_id, "sender-1")
            .await
            .expect("context");

        assert_eq!(ctx.current_agent_type.as_deref(), Some("codex"));
        assert_eq!(message.title.as_deref(), Some("Agent Selected"));
    }

    #[tokio::test]
    async fn sessions_in_general_topic_do_not_mark_sender_context_session_current() {
        let db = fresh_in_memory_db().await;
        let channel_id = seed_chat_channel(&db).await;
        let folder_id = seed_folder(&db, "/tmp/codeg-topic-general").await;
        let legacy_conv = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let _other_conv = seed_conversation(&db, folder_id, AgentType::OpenCode).await;
        sender_context_service::update_folder(&db.conn, channel_id, "sender-1", Some(folder_id))
            .await
            .expect("folder context");
        sender_context_service::update_session(
            &db.conn,
            channel_id,
            "sender-1",
            Some(legacy_conv),
            Some("legacy-connection".to_string()),
        )
        .await
        .expect("session context");

        let message = handle_sessions(
            &db.conn,
            channel_id,
            "sender-1",
            &ChannelMessageTarget::telegram_general(channel_id, "-100123"),
            Lang::En,
            "/",
        )
        .await;

        assert!(!message.body.contains("[*]"));
    }

    #[tokio::test]
    async fn sessions_in_forum_topic_mark_bound_conversation_not_sender_context() {
        let db = fresh_in_memory_db().await;
        let channel_id = seed_chat_channel(&db).await;
        let folder_id = seed_folder(&db, "/tmp/codeg-topic-bound").await;
        let legacy_conv = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let topic_conv = seed_conversation(&db, folder_id, AgentType::OpenCode).await;
        sender_context_service::update_folder(&db.conn, channel_id, "sender-1", Some(folder_id))
            .await
            .expect("folder context");
        sender_context_service::update_session(
            &db.conn,
            channel_id,
            "sender-1",
            Some(legacy_conv),
            Some("legacy-connection".to_string()),
        )
        .await
        .expect("session context");
        let target = ChannelMessageTarget::telegram_forum_topic(channel_id, "-100123", "2");
        thread_binding_service::upsert_for_target(
            &db.conn,
            &target,
            "telegram",
            topic_conv,
            Some("topic-connection".to_string()),
            "sender-1",
            Some("Topic session".to_string()),
        )
        .await
        .expect("thread binding");

        let message =
            handle_sessions(&db.conn, channel_id, "sender-1", &target, Lang::En, "/").await;

        assert!(message.body.contains(&format!("(#{topic_conv}) [*]")));
        assert!(!message.body.contains(&format!("(#{legacy_conv}) [*]")));
    }

    #[tokio::test]
    async fn resume_rejects_active_topic_even_for_same_conversation() {
        let db = fresh_in_memory_db().await;
        let channel_id = seed_chat_channel(&db).await;
        let folder_id = seed_folder(&db, "/tmp/codeg-topic-resume-active").await;
        let conv_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let target = ChannelMessageTarget::telegram_forum_topic(channel_id, "-100123", "2");
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));
        bridge.lock().await.register(
            "active-connection".to_string(),
            ActiveSession {
                channel_id,
                sender_id: "sender-1".to_string(),
                target: target.clone(),
                conversation_id: conv_id,
                connection_id: "active-connection".to_string(),
                agent_type: AgentType::Codex,
                content_buffer: String::new(),
                tool_calls: Vec::new(),
                tool_call_inputs: std::collections::HashMap::new(),
                delegation_rendered: std::collections::HashSet::new(),
                last_flushed: Instant::now(),
                pending_prompt: None,
                permission_pending: None,
            },
        );

        let message = handle_resume(
            &db.conn,
            &conv_id.to_string(),
            channel_id,
            "sender-1",
            &target,
            &ChatChannelManager::new(),
            &ConnectionManager::new(),
            &EventEmitter::Noop,
            &bridge,
            Lang::En,
            "/",
        )
        .await;

        assert!(message.body.contains("already has an active session"));
        assert_eq!(bridge.lock().await.all_sessions().count(), 1);
    }

    #[tokio::test]
    async fn permission_response_clears_stale_topic_binding_connection() {
        let db = fresh_in_memory_db().await;
        let channel_id = seed_chat_channel(&db).await;
        let folder_id = seed_folder(&db, "/tmp/codeg-topic-stale-permission").await;
        let conv_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let target = ChannelMessageTarget::telegram_forum_topic(channel_id, "-100123", "2");
        let binding = thread_binding_service::upsert_for_target(
            &db.conn,
            &target,
            "telegram",
            conv_id,
            Some("missing-connection".to_string()),
            "sender-1",
            Some("Topic session".to_string()),
        )
        .await
        .expect("thread binding");
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));

        let message = handle_permission_response(
            true,
            false,
            &db.conn,
            channel_id,
            "sender-1",
            &target,
            &ConnectionManager::new(),
            &bridge,
            Lang::En,
        )
        .await;
        let refreshed = thread_binding_service::get_by_target(&db.conn, &target)
            .await
            .expect("load binding")
            .expect("binding exists");

        assert_eq!(refreshed.id, binding.id);
        assert!(refreshed.connection_id.is_none());
        assert!(message.body.contains("No active session"));
    }
}
