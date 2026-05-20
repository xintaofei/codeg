use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::conversations as conv_commands;
use crate::db::service::{conversation_service, folder_service, import_service};
use crate::models::*;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListAllConversationsParams {
    pub folder_ids: Option<Vec<i32>>,
    pub agent_type: Option<AgentType>,
    pub search: Option<String>,
    pub sort_by: Option<String>,
    pub status: Option<String>,
}

pub async fn list_all_conversations(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ListAllConversationsParams>,
) -> Result<Json<Vec<DbConversationSummary>>, AppCommandError> {
    let db = &state.db;
    let result = conversation_service::list_all(
        &db.conn,
        params.folder_ids,
        params.agent_type,
        params.search,
        params.sort_by,
        params.status,
    )
    .await
    .map_err(AppCommandError::from)?;
    Ok(Json(result))
}

pub async fn list_opened_tabs(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<OpenedTab>>, AppCommandError> {
    use crate::db::service::tab_service;
    let db = &state.db;
    let result = tab_service::list_all_tabs(&db.conn)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOpenedTabsParams {
    pub items: Vec<OpenedTab>,
}

pub async fn save_opened_tabs(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<SaveOpenedTabsParams>,
) -> Result<Json<()>, AppCommandError> {
    use crate::db::service::tab_service;
    let db = &state.db;
    tab_service::save_all_tabs(&db.conn, params.items)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListConversationsParams {
    pub agent_type: Option<AgentType>,
    pub search: Option<String>,
    pub sort_by: Option<String>,
    pub folder_path: Option<String>,
}

pub async fn list_conversations(
    Json(params): Json<ListConversationsParams>,
) -> Result<Json<Vec<ConversationSummary>>, AppCommandError> {
    let result = conv_commands::list_conversations(
        params.agent_type,
        params.search,
        params.sort_by,
        params.folder_path,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetConversationParams {
    pub agent_type: AgentType,
    pub conversation_id: String,
}

pub async fn get_conversation(
    Json(params): Json<GetConversationParams>,
) -> Result<Json<ConversationDetail>, AppCommandError> {
    let result = conv_commands::get_conversation(params.agent_type, params.conversation_id).await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetFolderConversationParams {
    pub conversation_id: i32,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub latest: Option<bool>,
}

pub async fn get_folder_conversation(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<GetFolderConversationParams>,
) -> Result<Json<DbConversationDetail>, AppCommandError> {
    let db = &state.db;
    let result = conv_commands::get_folder_conversation_core(
        &db.conn,
        params.conversation_id,
        params.offset,
        params.limit,
        params.latest,
    )
    .await?;
    Ok(Json(result))
}

pub async fn list_folders() -> Result<Json<Vec<FolderInfo>>, AppCommandError> {
    let result = conv_commands::list_folders().await?;
    Ok(Json(result))
}

pub async fn get_stats() -> Result<Json<AgentStats>, AppCommandError> {
    let result = conv_commands::get_stats().await?;
    Ok(Json(result))
}

pub async fn get_sidebar_data() -> Result<Json<SidebarData>, AppCommandError> {
    let result = conv_commands::get_sidebar_data().await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportLocalConversationsParams {
    pub folder_id: i32,
}

pub async fn import_local_conversations(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ImportLocalConversationsParams>,
) -> Result<Json<ImportResult>, AppCommandError> {
    let db = &state.db;
    let folder = folder_service::get_folder_by_id(&db.conn, params.folder_id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Folder not found"))?;
    let result =
        import_service::import_local_conversations(&db.conn, params.folder_id, &folder.path)
            .await
            .map_err(AppCommandError::from)?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConversationParams {
    pub folder_id: i32,
    pub agent_type: AgentType,
    pub title: Option<String>,
}

pub async fn create_conversation(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<CreateConversationParams>,
) -> Result<Json<i32>, AppCommandError> {
    let db = &state.db;
    let result = conv_commands::create_conversation_core(
        &db.conn,
        params.folder_id,
        params.agent_type,
        params.title,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConversationStatusParams {
    pub conversation_id: i32,
    pub status: String,
}

pub async fn update_conversation_status(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateConversationStatusParams>,
) -> Result<Json<()>, AppCommandError> {
    let db = &state.db;
    let status_enum: crate::db::entities::conversation::ConversationStatus =
        serde_json::from_value(serde_json::Value::String(params.status)).map_err(|e| {
            AppCommandError::invalid_input("Invalid conversation status").with_detail(e.to_string())
        })?;
    conversation_service::update_status(&db.conn, params.conversation_id, status_enum)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConversationTitleParams {
    pub conversation_id: i32,
    pub title: String,
}

pub async fn update_conversation_title(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateConversationTitleParams>,
) -> Result<Json<()>, AppCommandError> {
    let db = &state.db;
    conversation_service::update_title(&db.conn, params.conversation_id, params.title)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConversationParams {
    pub conversation_id: i32,
}

pub async fn delete_conversation(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<DeleteConversationParams>,
) -> Result<Json<()>, AppCommandError> {
    let db = &state.db;
    conversation_service::soft_delete(&db.conn, params.conversation_id)
        .await
        .map_err(AppCommandError::from)?;
    Ok(Json(()))
}
