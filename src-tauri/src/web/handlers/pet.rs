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
    ImportCodexPetsRequest, ImportCodexPetsResult, ImportablePet, NewPetInput, PetDetail,
    PetSpriteAsset, PetSummary, PetWindowConfig, PetWindowStatePatch,
};

pub async fn pet_list() -> Result<Json<Vec<PetSummary>>, AppCommandError> {
    pet_commands::pet_list_core().await.map(Json)
}

pub async fn pet_get(
    Json(params): Json<PetIdParams>,
) -> Result<Json<PetDetail>, AppCommandError> {
    pet_commands::pet_get_core(params.id).await.map(Json)
}

pub async fn pet_read_spritesheet(
    Json(params): Json<PetIdParams>,
) -> Result<Json<PetSpriteAsset>, AppCommandError> {
    pet_commands::pet_read_spritesheet_core(params.id)
        .await
        .map(Json)
}

pub async fn pet_add(
    Json(input): Json<NewPetInput>,
) -> Result<Json<PetSummary>, AppCommandError> {
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

pub async fn pet_list_importable_codex(
) -> Result<Json<Vec<ImportablePet>>, AppCommandError> {
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
    pet_commands::pet_set_active_core(&state.db.conn, params.pet_id)
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
