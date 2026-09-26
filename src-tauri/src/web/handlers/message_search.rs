use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::message_search as search_commands;
use crate::db::service::message_search_service::MessageSearchHit;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageSearchParams {
    pub query: String,
    pub limit: Option<u32>,
}

pub async fn message_search(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<MessageSearchParams>,
) -> Result<Json<Vec<MessageSearchHit>>, AppCommandError> {
    Ok(Json(
        search_commands::message_search_core(&state.db.conn, &params.query, params.limit).await?,
    ))
}
