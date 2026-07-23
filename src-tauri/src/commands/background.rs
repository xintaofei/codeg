//! Tauri + Axum command surface for the workspace background image.
//!
//! All filesystem operations live in `crate::backgrounds`; this module owns the
//! thin double-mode wrappers that offload the blocking I/O and surface it as
//! `AppCommandError`. All three commands are **stateless** (disk-only, no DB /
//! `AppState`), like `pet_read_spritesheet` / `pet_add` / `pet_replace_sprite`.

use crate::app_error::AppCommandError;
use crate::backgrounds;
use crate::models::background::BackgroundAsset;

// ─── core ops (filesystem) ──────────────────────────────────────────────

pub async fn background_read_core() -> Result<Option<BackgroundAsset>, AppCommandError> {
    tokio::task::spawn_blocking(backgrounds::read_background)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?
}

pub async fn background_set_core(image_base64: String) -> Result<(), AppCommandError> {
    tokio::task::spawn_blocking(move || backgrounds::set_background(&image_base64))
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?
}

pub async fn background_clear_core() -> Result<(), AppCommandError> {
    tokio::task::spawn_blocking(backgrounds::clear_background)
        .await
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?
}

// ─── web-handler param struct ───────────────────────────────────────────

/// Web-mode JSON body for `background_set`. The Tauri command takes a flat
/// `imageBase64` scalar (auto snake_case-translated on the way in); the Axum
/// handler needs a named struct to deserialize the same payload.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundSetParams {
    pub image_base64: String,
}

// ─── tauri command wrappers ─────────────────────────────────────────────

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn background_read() -> Result<Option<BackgroundAsset>, AppCommandError> {
    background_read_core().await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn background_set(image_base64: String) -> Result<(), AppCommandError> {
    background_set_core(image_base64).await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn background_clear() -> Result<(), AppCommandError> {
    background_clear_core().await
}
