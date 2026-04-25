use chrono::Utc;
use std::collections::HashMap;
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

fn conv_to_summary(
    r: conversation::Model,
    folder: Option<&folder::Model>,
) -> DbConversationSummary {
    let status = serde_json::to_value(&r.status)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| format!("{:?}", r.status));
    DbConversationSummary {
        id: r.id,
        folder_id: r.folder_id,
        folder_name: folder.map(|value| value.name.clone()),
        folder_path: folder.map(|value| value.path.clone()),
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
    let folder = folder::Entity::find_by_id(conv.folder_id).one(conn).await?;

    Ok(conv_to_summary(conv, folder.as_ref()))
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
    let folder_row = folder::Entity::find_by_id(folder_id).one(conn).await?;
    let mut summaries: Vec<DbConversationSummary> = rows
        .into_iter()
        .map(|row| conv_to_summary(row, folder_row.as_ref()))
        .collect();

    apply_search_and_sort(&mut summaries, search, sort_by, false);
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
    let folder_rows = folder::Entity::find()
        .filter(folder::Column::DeletedAt.is_null())
        .all(conn)
        .await?;
    let folder_map: HashMap<i32, folder::Model> =
        folder_rows.into_iter().map(|row| (row.id, row)).collect();

    let mut summaries = rows
        .into_iter()
        .map(|row| conv_to_summary(row.clone(), folder_map.get(&row.folder_id)))
        .collect::<Vec<_>>();
    apply_search_and_sort(&mut summaries, search, sort_by, true);
    Ok(summaries)
}

fn apply_search_and_sort(
    rows: &mut Vec<DbConversationSummary>,
    search: Option<String>,
    sort_by: Option<String>,
    use_updated_at: bool,
) {
    let search = search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase);

    if let Some(query) = search.as_deref() {
        let mut scored = Vec::with_capacity(rows.len());
        for row in rows.drain(..) {
            if let Some(score) = search_score(&row, query) {
                scored.push((score, row));
            }
        }

        scored.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.updated_at.cmp(&left.1.updated_at))
        });
        rows.extend(scored.into_iter().map(|(_, row)| row));
        return;
    }

    match sort_by.as_deref() {
        Some("oldest") => rows.sort_by(|left, right| {
            if use_updated_at {
                left.updated_at.cmp(&right.updated_at)
            } else {
                left.created_at.cmp(&right.created_at)
            }
        }),
        _ => rows.sort_by(|left, right| {
            if use_updated_at {
                right.updated_at.cmp(&left.updated_at)
            } else {
                right.created_at.cmp(&left.created_at)
            }
        }),
    }
}

fn search_score(row: &DbConversationSummary, query: &str) -> Option<i32> {
    let mut score = 0i32;

    score = score.max(field_score(row.title.as_deref(), query, 120, 100, 75));
    score = score.max(field_score(row.external_id.as_deref(), query, 110, 95, 70));
    score = score.max(field_score(row.folder_name.as_deref(), query, 90, 80, 60));
    score = score.max(field_score(row.folder_path.as_deref(), query, 85, 70, 55));
    score = score.max(field_score(row.model.as_deref(), query, 70, 60, 45));
    score = score.max(field_score(row.git_branch.as_deref(), query, 65, 55, 40));

    if score == 0 { None } else { Some(score) }
}

fn field_score(
    candidate: Option<&str>,
    query: &str,
    exact_score: i32,
    prefix_score: i32,
    contains_score: i32,
) -> i32 {
    let Some(candidate) = candidate.map(str::trim).filter(|value| !value.is_empty()) else {
        return 0;
    };
    let lowered = candidate.to_lowercase();
    if lowered == query {
        exact_score
    } else if lowered.starts_with(query) {
        prefix_score
    } else if lowered.contains(query) {
        contains_score
    } else {
        0
    }
}
