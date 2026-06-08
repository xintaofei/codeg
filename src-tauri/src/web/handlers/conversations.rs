use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::conversations as conv_commands;
use crate::models::*;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListAllConversationsParams {
    pub folder_ids: Option<Vec<i32>>,
    pub agent_type: Option<AgentType>,
    pub search: Option<String>,
    pub sort_by: Option<String>,
    pub status: Option<String>,
    pub include_children: Option<bool>,
}

pub async fn list_all_conversations(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ListAllConversationsParams>,
) -> Result<Json<Vec<DbConversationSummary>>, AppCommandError> {
    Ok(Json(
        conv_commands::list_all_conversations_core(
            &state.db.conn,
            params.folder_ids,
            params.agent_type,
            params.search,
            params.sort_by,
            params.status,
            params.include_children.unwrap_or(false),
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListChildConversationsParams {
    pub parent_conversation_id: i32,
}

pub async fn list_child_conversations(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ListChildConversationsParams>,
) -> Result<Json<Vec<DbConversationSummary>>, AppCommandError> {
    Ok(Json(
        conv_commands::list_child_conversations_core(&state.db.conn, params.parent_conversation_id)
            .await?,
    ))
}

pub async fn list_opened_tabs(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<OpenedTabsSnapshot>, AppCommandError> {
    Ok(Json(
        conv_commands::list_opened_tabs_core(&state.db.conn).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOpenedTabsParams {
    pub items: Vec<OpenedTab>,
    pub expected_version: i64,
    pub origin: String,
}

pub async fn save_opened_tabs(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<SaveOpenedTabsParams>,
) -> Result<Json<SaveTabsOutcome>, AppCommandError> {
    Ok(Json(
        conv_commands::save_opened_tabs_core(
            &state.db.conn,
            &state.emitter,
            params.items,
            params.expected_version,
            params.origin,
        )
        .await?,
    ))
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
}

pub async fn get_folder_conversation(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<GetFolderConversationParams>,
) -> Result<Json<DbConversationDetail>, AppCommandError> {
    let db = &state.db;
    let result = conv_commands::get_folder_conversation_with_live_core(
        &db.conn,
        &state.connection_manager,
        params.conversation_id,
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
    Ok(Json(
        conv_commands::import_local_conversations_core(&state.db.conn, params.folder_id).await?,
    ))
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
    conv_commands::emit_conversation_upsert(&state.emitter, &db.conn, result).await;
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
    conv_commands::update_conversation_status_core(
        &state.db.conn,
        params.conversation_id,
        params.status,
    )
    .await?;
    conv_commands::emit_conversation_upsert(&state.emitter, &state.db.conn, params.conversation_id)
        .await;
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
    conv_commands::update_conversation_title_core(
        &state.db.conn,
        params.conversation_id,
        params.title,
    )
    .await?;
    conv_commands::emit_conversation_upsert(&state.emitter, &state.db.conn, params.conversation_id)
        .await;
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
    conv_commands::delete_conversation_core(&state.db.conn, params.conversation_id).await?;
    conv_commands::emit_conversation_deleted(&state.emitter, params.conversation_id);
    conv_commands::cleanup_tabs_for_deleted_conversation(
        &state.emitter,
        &state.db.conn,
        params.conversation_id,
    )
    .await;
    Ok(Json(()))
}
