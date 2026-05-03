use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder, Set,
};

use crate::db::entities::{conversation, folder};
use crate::db::error::DbError;
use crate::models::{AgentType, DbConversationSummary};

pub async fn create(
    conn: &DatabaseConnection,
    folder_id: i32,
    agent_type: AgentType,
    title: Option<String>,
    git_branch: Option<String>,
) -> Result<conversation::Model, DbError> {
    let at_str = serde_json::to_value(agent_type)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let now = Utc::now();
    let model = conversation::ActiveModel {
        id: NotSet,
        folder_id: Set(folder_id),
        title: Set(title),
        agent_type: Set(at_str),
        status: Set(conversation::ConversationStatus::InProgress),
        model: Set(None),
        git_branch: Set(git_branch),
        external_id: Set(None),
        parent_id: Set(None),
        message_count: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
        deleted_at: Set(None),
    };
    Ok(model.insert(conn).await?)
}

pub async fn update_status(
    conn: &DatabaseConnection,
    conversation_id: i32,
    status: conversation::ConversationStatus,
) -> Result<(), DbError> {
    let conv = conversation::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("Conversation not found: {conversation_id}")))?;
    let mut active: conversation::ActiveModel = conv.into();
    active.status = Set(status);
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}

/// Conditional status transition (CAS): write `new_status` only if the row's
/// current `status` equals `expected`. Returns `true` when the row was
/// updated. Used by the lifecycle subscriber on disconnect/error so a
/// concurrent user-driven `completed` (or a prior `pending_review` from
/// `TurnComplete`) cannot be silently overwritten.
pub async fn update_status_if(
    conn: &DatabaseConnection,
    conversation_id: i32,
    expected: conversation::ConversationStatus,
    new_status: conversation::ConversationStatus,
) -> Result<bool, DbError> {
    use sea_orm::sea_query::Expr;
    let result = conversation::Entity::update_many()
        .col_expr(conversation::Column::Status, Expr::value(new_status))
        .col_expr(conversation::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(conversation::Column::Id.eq(conversation_id))
        .filter(conversation::Column::Status.eq(expected))
        .exec(conn)
        .await?;
    Ok(result.rows_affected > 0)
}

pub async fn update_title(
    conn: &DatabaseConnection,
    conversation_id: i32,
    title: String,
) -> Result<(), DbError> {
    let conv = conversation::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("Conversation not found: {conversation_id}")))?;
    let mut active: conversation::ActiveModel = conv.into();
    active.title = Set(Some(title));
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}

pub async fn update_external_id(
    conn: &DatabaseConnection,
    conversation_id: i32,
    external_id: String,
) -> Result<(), DbError> {
    let conv = conversation::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("Conversation not found: {conversation_id}")))?;
    let mut active: conversation::ActiveModel = conv.into();
    active.external_id = Set(Some(external_id));
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}

pub async fn update_external_id_if_missing(
    conn: &DatabaseConnection,
    conversation_id: i32,
    external_id: String,
) -> Result<bool, DbError> {
    use sea_orm::sea_query::Expr;
    let result = conversation::Entity::update_many()
        .col_expr(conversation::Column::ExternalId, Expr::value(external_id))
        .col_expr(conversation::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(conversation::Column::Id.eq(conversation_id))
        .filter(conversation::Column::DeletedAt.is_null())
        .filter(conversation::Column::ExternalId.is_null())
        .exec(conn)
        .await?;
    Ok(result.rows_affected > 0)
}

pub async fn soft_delete(conn: &DatabaseConnection, conversation_id: i32) -> Result<(), DbError> {
    let conv = conversation::Entity::find_by_id(conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("Conversation not found: {conversation_id}")))?;
    let mut active: conversation::ActiveModel = conv.into();
    active.deleted_at = Set(Some(Utc::now()));
    active.update(conn).await?;
    Ok(())
}

fn parse_agent_type(s: &str) -> AgentType {
    match serde_json::from_value(serde_json::Value::String(s.to_string())) {
        Ok(at) => at,
        Err(_) => {
            // DB has a value the enum does not recognise (manual edit or removed variant).
            // Fall back to ClaudeCode so the row stays readable, but log so resume-as-wrong-agent
            // regressions are traceable.
            eprintln!(
                "[conversation_service] unknown agent_type {s:?} in DB, falling back to ClaudeCode"
            );
            AgentType::ClaudeCode
        }
    }
}

fn conv_to_summary(r: conversation::Model) -> DbConversationSummary {
    let status = serde_json::to_value(&r.status)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| format!("{:?}", r.status));
    DbConversationSummary {
        id: r.id,
        folder_id: r.folder_id,
        title: r.title,
        agent_type: parse_agent_type(&r.agent_type),
        status,
        model: r.model,
        git_branch: r.git_branch,
        external_id: r.external_id,
        message_count: r.message_count as u32,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }
}

pub async fn get_by_id(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<DbConversationSummary, DbError> {
    let conv = conversation::Entity::find_by_id(conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("Conversation not found: {conversation_id}")))?;

    Ok(conv_to_summary(conv))
}

pub async fn list_by_folder(
    conn: &DatabaseConnection,
    folder_id: i32,
    agent_type: Option<AgentType>,
    search: Option<String>,
    sort_by: Option<String>,
    status: Option<String>,
) -> Result<Vec<DbConversationSummary>, DbError> {
    let mut query = conversation::Entity::find()
        .filter(conversation::Column::FolderId.eq(folder_id))
        .filter(conversation::Column::DeletedAt.is_null());

    // Filter by agent_type
    if let Some(ref at) = agent_type {
        let at_str = serde_json::to_value(at)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        query = query.filter(conversation::Column::AgentType.eq(at_str));
    }

    // Search by title
    if let Some(ref s) = search {
        if !s.is_empty() {
            query = query.filter(conversation::Column::Title.contains(s));
        }
    }

    // Filter by status
    if let Some(ref st) = status {
        if let Ok(status_enum) = serde_json::from_value::<conversation::ConversationStatus>(
            serde_json::Value::String(st.clone()),
        ) {
            query = query.filter(conversation::Column::Status.eq(status_enum));
        }
    }

    // Sort
    query = match sort_by.as_deref() {
        Some("oldest") => query.order_by_asc(conversation::Column::CreatedAt),
        _ => query.order_by_desc(conversation::Column::CreatedAt),
    };

    let rows = query.all(conn).await?;

    let summaries: Vec<DbConversationSummary> = rows.into_iter().map(conv_to_summary).collect();

    Ok(summaries)
}

/// List conversations across folders. When `folder_ids` is `None`, queries all
/// When `folder_ids` is provided, results are scoped to that set. Otherwise
/// returns conversations across every non-deleted folder (open or not).
pub async fn list_all(
    conn: &DatabaseConnection,
    folder_ids: Option<Vec<i32>>,
    agent_type: Option<AgentType>,
    search: Option<String>,
    sort_by: Option<String>,
    status: Option<String>,
) -> Result<Vec<DbConversationSummary>, DbError> {
    let mut query = conversation::Entity::find().filter(conversation::Column::DeletedAt.is_null());

    match folder_ids {
        Some(ids) if !ids.is_empty() => {
            query = query.filter(conversation::Column::FolderId.is_in(ids));
        }
        _ => {
            // Exclude conversations whose folder was soft-deleted.
            let active_folder_ids: Vec<i32> = folder::Entity::find()
                .filter(folder::Column::DeletedAt.is_null())
                .all(conn)
                .await?
                .into_iter()
                .map(|m| m.id)
                .collect();
            if active_folder_ids.is_empty() {
                return Ok(Vec::new());
            }
            query = query.filter(conversation::Column::FolderId.is_in(active_folder_ids));
        }
    }

    if let Some(ref at) = agent_type {
        let at_str = serde_json::to_value(at)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        query = query.filter(conversation::Column::AgentType.eq(at_str));
    }

    if let Some(ref s) = search {
        if !s.is_empty() {
            query = query.filter(conversation::Column::Title.contains(s));
        }
    }

    if let Some(ref st) = status {
        if let Ok(status_enum) = serde_json::from_value::<conversation::ConversationStatus>(
            serde_json::Value::String(st.clone()),
        ) {
            query = query.filter(conversation::Column::Status.eq(status_enum));
        }
    }

    query = match sort_by.as_deref() {
        Some("oldest") => query.order_by_asc(conversation::Column::UpdatedAt),
        _ => query.order_by_desc(conversation::Column::UpdatedAt),
    };

    let rows = query.all(conn).await?;
    Ok(rows.into_iter().map(conv_to_summary).collect())
}
