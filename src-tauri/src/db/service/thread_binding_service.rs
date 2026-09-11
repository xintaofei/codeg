use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, Set,
};

use crate::chat_channel::types::ChannelMessageTarget;
use crate::db::entities::chat_channel_thread_binding;
use crate::db::error::DbError;

pub async fn get_by_target(
    conn: &DatabaseConnection,
    target: &ChannelMessageTarget,
) -> Result<Option<chat_channel_thread_binding::Model>, DbError> {
    let Some(chat_id) = target.chat_id.as_deref() else {
        return Ok(None);
    };
    let Some(thread_key) = target.thread_key.as_deref() else {
        return Ok(None);
    };
    let Some(thread_kind) = target.thread_kind.as_deref() else {
        return Ok(None);
    };

    Ok(chat_channel_thread_binding::Entity::find()
        .filter(chat_channel_thread_binding::Column::ChannelId.eq(target.channel_id))
        .filter(chat_channel_thread_binding::Column::ThreadKind.eq(thread_kind))
        .filter(chat_channel_thread_binding::Column::ChatId.eq(chat_id))
        .filter(chat_channel_thread_binding::Column::ThreadKey.eq(thread_key))
        .one(conn)
        .await?)
}

pub async fn list_by_conversation(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<chat_channel_thread_binding::Model>, DbError> {
    Ok(chat_channel_thread_binding::Entity::find()
        .filter(chat_channel_thread_binding::Column::ConversationId.eq(conversation_id))
        .all(conn)
        .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn upsert_for_target(
    conn: &DatabaseConnection,
    target: &ChannelMessageTarget,
    channel_type: &str,
    conversation_id: i32,
    connection_id: Option<String>,
    created_by_sender_id: &str,
    display_title: Option<String>,
) -> Result<chat_channel_thread_binding::Model, DbError> {
    let chat_id = target
        .chat_id
        .as_deref()
        .ok_or_else(|| DbError::Validation("thread binding requires chat_id".to_string()))?;
    let thread_key = target
        .thread_key
        .as_deref()
        .ok_or_else(|| DbError::Validation("thread binding requires thread_key".to_string()))?;
    let thread_kind = target
        .thread_kind
        .as_deref()
        .ok_or_else(|| DbError::Validation("thread binding requires thread_kind".to_string()))?;
    let now = Utc::now();

    if let Some(existing) = get_by_target(conn, target).await? {
        let mut active = existing.into_active_model();
        active.channel_type = Set(channel_type.to_string());
        active.conversation_id = Set(conversation_id);
        active.connection_id = Set(connection_id);
        active.created_by_sender_id = Set(created_by_sender_id.to_string());
        active.display_title = Set(display_title);
        active.updated_at = Set(now);
        return Ok(active.update(conn).await?);
    }

    let active = chat_channel_thread_binding::ActiveModel {
        id: NotSet,
        channel_id: Set(target.channel_id),
        channel_type: Set(channel_type.to_string()),
        chat_id: Set(chat_id.to_string()),
        thread_key: Set(thread_key.to_string()),
        thread_kind: Set(thread_kind.to_string()),
        conversation_id: Set(conversation_id),
        connection_id: Set(connection_id),
        created_by_sender_id: Set(created_by_sender_id.to_string()),
        display_title: Set(display_title),
        title_sync_enabled: Set(true),
        provider_payload_json: Set(target
            .provider_payload
            .as_ref()
            .map(serde_json::Value::to_string)),
        created_at: Set(now),
        updated_at: Set(now),
    };
    Ok(active.insert(conn).await?)
}

pub async fn update_display_title(
    conn: &DatabaseConnection,
    id: i32,
    display_title: String,
) -> Result<chat_channel_thread_binding::Model, DbError> {
    let model = chat_channel_thread_binding::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("thread binding {id}")))?;
    let mut active = model.into_active_model();
    active.display_title = Set(Some(display_title));
    active.updated_at = Set(Utc::now());
    Ok(active.update(conn).await?)
}

pub async fn clear_connection(
    conn: &DatabaseConnection,
    id: i32,
) -> Result<chat_channel_thread_binding::Model, DbError> {
    let model = chat_channel_thread_binding::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("thread binding {id}")))?;
    let mut active = model.into_active_model();
    active.connection_id = Set(None);
    active.updated_at = Set(Utc::now());
    Ok(active.update(conn).await?)
}
