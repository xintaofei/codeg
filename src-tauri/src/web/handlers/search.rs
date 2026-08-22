use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::search as search_commands;
use crate::models::{AgentType, DbConversationSearchResult, SearchIndexStatus};

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchConversationsParams {
    pub folder_ids: Option<Vec<i32>>,
    pub agent_type: Option<AgentType>,
    pub query: String,
    pub limit: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSearchSettingsParams {
    pub enabled: bool,
    pub user_mode: String,
}

pub async fn search_conversations(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<SearchConversationsParams>,
) -> Result<Json<Vec<DbConversationSearchResult>>, AppCommandError> {
    Ok(Json(
        search_commands::search_conversations_core(
            &state.db.conn,
            params.folder_ids,
            params.agent_type,
            params.query,
            params.limit,
        )
        .await?,
    ))
}

pub async fn get_search_index_status(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<SearchIndexStatus>, AppCommandError> {
    Ok(Json(
        search_commands::get_search_index_status_core(&state.db.conn).await?,
    ))
}

pub async fn set_search_settings(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<SetSearchSettingsParams>,
) -> Result<Json<()>, AppCommandError> {
    search_commands::set_search_settings_core(&state.db.conn, params.enabled, params.user_mode)
        .await?;
    if let Some(indexer) = &state.search_indexer {
        indexer.request_mode_sync();
    }
    Ok(Json(()))
}
