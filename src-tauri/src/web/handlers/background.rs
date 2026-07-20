//! Axum handlers mirroring `commands::background`. All three are stateless
//! (disk-only), so none take `Extension<Arc<AppState>>`.

use axum::Json;

use crate::app_error::AppCommandError;
use crate::commands::background as background_commands;
use crate::commands::background::BackgroundSetParams;
use crate::models::background::BackgroundAsset;

pub async fn background_read() -> Result<Json<Option<BackgroundAsset>>, AppCommandError> {
    background_commands::background_read_core().await.map(Json)
}

pub async fn background_set(
    Json(params): Json<BackgroundSetParams>,
) -> Result<Json<()>, AppCommandError> {
    background_commands::background_set_core(params.image_base64)
        .await
        .map(Json)
}

pub async fn background_clear() -> Result<Json<()>, AppCommandError> {
    background_commands::background_clear_core().await.map(Json)
}
