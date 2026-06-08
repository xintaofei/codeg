//! Axum handlers mirroring `commands::pet`. The window management commands
//! (open/close) are not exposed here — they are pure Tauri operations and
//! have no equivalent in standalone-server mode.

use std::sync::Arc;

use axum::{extract::Extension, Json};

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::pet as pet_commands;
use crate::commands::pet::{
    PetCodexImportAvailability, PetIdParams, PetReplaceSpriteParams, PetSetActiveParams,
    PetUpdateMetaParams,
};
use crate::models::pet::{
    ImportCodexPetsRequest, ImportCodexPetsResult, ImportablePet, NewPetInput, PetCelebrationKind,
    PetDetail, PetSessionsPayload, PetSpriteAsset, PetState, PetSummary, PetWindowConfig,
    PetWindowStatePatch,
};
use crate::pets::marketplace::{
    MarketplaceInstallRequest, MarketplaceInstallResponse, MarketplaceListParams,
    MarketplaceListResponse,
};

pub async fn pet_list() -> Result<Json<Vec<PetSummary>>, AppCommandError> {
    pet_commands::pet_list_core().await.map(Json)
}

pub async fn pet_get(Json(params): Json<PetIdParams>) -> Result<Json<PetDetail>, AppCommandError> {
    pet_commands::pet_get_core(params.id).await.map(Json)
}

pub async fn pet_read_spritesheet(
    Json(params): Json<PetIdParams>,
) -> Result<Json<PetSpriteAsset>, AppCommandError> {
    pet_commands::pet_read_spritesheet_core(params.id)
        .await
        .map(Json)
}

pub async fn pet_add(Json(input): Json<NewPetInput>) -> Result<Json<PetSummary>, AppCommandError> {
    pet_commands::pet_add_core(input).await.map(Json)
}

pub async fn pet_update_meta(
    Json(params): Json<PetUpdateMetaParams>,
) -> Result<Json<PetSummary>, AppCommandError> {
    pet_commands::pet_update_meta_core(params.id, params.patch)
        .await
        .map(Json)
}

pub async fn pet_replace_sprite(
    Json(params): Json<PetReplaceSpriteParams>,
) -> Result<Json<()>, AppCommandError> {
    pet_commands::pet_replace_sprite_core(params.id, params.spritesheet_base64)
        .await
        .map(Json)
}

pub async fn pet_delete(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<PetIdParams>,
) -> Result<Json<()>, AppCommandError> {
    pet_commands::pet_delete_core(&state.db.conn, params.id)
        .await
        .map(Json)
}

pub async fn pet_list_importable_codex() -> Result<Json<Vec<ImportablePet>>, AppCommandError> {
    pet_commands::pet_list_importable_codex_core()
        .await
        .map(Json)
}

pub async fn pet_import_codex(
    Json(request): Json<ImportCodexPetsRequest>,
) -> Result<Json<ImportCodexPetsResult>, AppCommandError> {
    pet_commands::pet_import_codex_core(request).await.map(Json)
}

pub async fn pet_codex_import_available(
) -> Result<Json<PetCodexImportAvailability>, AppCommandError> {
    pet_commands::pet_codex_import_available_core()
        .await
        .map(Json)
}

pub async fn pet_get_settings(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<PetWindowConfig>, AppCommandError> {
    pet_commands::pet_get_settings_core(&state.db.conn)
        .await
        .map(Json)
}

pub async fn pet_set_active(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<PetSetActiveParams>,
) -> Result<Json<PetWindowConfig>, AppCommandError> {
    pet_commands::pet_set_active_core(&state.db.conn, &state.emitter, params.pet_id)
        .await
        .map(Json)
}

pub async fn pet_save_window_state(
    Extension(state): Extension<Arc<AppState>>,
    Json(patch): Json<PetWindowStatePatch>,
) -> Result<Json<PetWindowConfig>, AppCommandError> {
    pet_commands::pet_save_window_state_core(&state.db.conn, patch)
        .await
        .map(Json)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetCelebrateParams {
    pub kind: PetCelebrationKind,
}

pub async fn pet_celebrate(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<PetCelebrateParams>,
) -> Result<Json<()>, AppCommandError> {
    pet_commands::pet_celebrate_core(&state.emitter, params.kind);
    Ok(Json(()))
}

pub async fn pet_marketplace_list(
    Json(params): Json<MarketplaceListParams>,
) -> Result<Json<MarketplaceListResponse>, AppCommandError> {
    pet_commands::pet_marketplace_list_core(params)
        .await
        .map(Json)
}

pub async fn pet_marketplace_install(
    Json(request): Json<MarketplaceInstallRequest>,
) -> Result<Json<MarketplaceInstallResponse>, AppCommandError> {
    pet_commands::pet_marketplace_install_core(request)
        .await
        .map(Json)
}

pub async fn pet_get_current_state(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<PetState>, AppCommandError> {
    Ok(Json(pet_commands::pet_get_current_state_core(
        &state.pet_state,
    )))
}

pub async fn pet_list_active_sessions(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<PetSessionsPayload>, AppCommandError> {
    pet_commands::pet_list_active_sessions_core(&state.connection_manager, &state.db.conn)
        .await
        .map(Json)
}
