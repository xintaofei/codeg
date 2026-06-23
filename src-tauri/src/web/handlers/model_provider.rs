use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::model_provider as mp_commands;
use crate::models::model_provider::{
    ImportCcSwitchModelProvidersRequest, ImportCcSwitchModelProvidersResult,
    ListImportableCcSwitchModelProvidersResult, ModelProviderInfo,
};

// ---------------------------------------------------------------------------
// Param structs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateModelProviderParams {
    pub name: String,
    pub api_url: String,
    pub api_key: String,
    pub agent_type: String,
    pub model: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateModelProviderParams {
    pub id: i32,
    pub name: Option<String>,
    pub api_url: Option<String>,
    pub api_key: Option<String>,
    pub agent_type: Option<String>,
    pub model: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProviderIdParams {
    pub id: i32,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn list_model_providers(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<ModelProviderInfo>>, AppCommandError> {
    let result = mp_commands::list_model_providers_core(&state.db).await?;
    Ok(Json(result))
}

pub async fn create_model_provider(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<CreateModelProviderParams>,
) -> Result<Json<ModelProviderInfo>, AppCommandError> {
    let result = mp_commands::create_model_provider_core(
        &state.db,
        params.name,
        params.api_url,
        params.api_key,
        params.agent_type,
        params.model,
    )
    .await?;
    Ok(Json(result))
}

pub async fn update_model_provider(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateModelProviderParams>,
) -> Result<Json<mp_commands::UpdateModelProviderResult>, AppCommandError> {
    let result = mp_commands::update_model_provider_and_refresh(
        &state.db,
        &state.connection_manager,
        &state.data_dir,
        params.id,
        params.name,
        params.api_url,
        params.api_key,
        params.agent_type,
        params.model,
        &state.emitter,
    )
    .await?;
    Ok(Json(result))
}

pub async fn delete_model_provider(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ModelProviderIdParams>,
) -> Result<Json<()>, AppCommandError> {
    mp_commands::delete_model_provider_core(&state.db, params.id).await?;
    Ok(Json(()))
}

pub async fn list_importable_cc_switch_model_providers(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<ListImportableCcSwitchModelProvidersResult>, AppCommandError> {
    let result = mp_commands::list_importable_cc_switch_model_providers_core(&state.db).await?;
    Ok(Json(result))
}

pub async fn import_cc_switch_model_providers(
    Extension(state): Extension<Arc<AppState>>,
    Json(request): Json<ImportCcSwitchModelProvidersRequest>,
) -> Result<Json<ImportCcSwitchModelProvidersResult>, AppCommandError> {
    let result = mp_commands::import_cc_switch_model_providers_core(&state.db, request).await?;
    Ok(Json(result))
}
