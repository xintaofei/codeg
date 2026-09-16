//! Axum handlers mirroring `commands::preset_gallery`. Both are stateless
//! (proxied fetch only), so neither takes `Extension<Arc<AppState>>`.

use axum::Json;

use crate::app_error::AppCommandError;
use crate::commands::preset_gallery as preset_gallery_commands;
use crate::commands::preset_gallery::{PresetGalleryIndexParams, PresetGalleryPresetParams};
use crate::preset_gallery::GalleryDocument;

pub async fn preset_gallery_fetch_index(
    Json(params): Json<PresetGalleryIndexParams>,
) -> Result<Json<GalleryDocument>, AppCommandError> {
    preset_gallery_commands::preset_gallery_fetch_index_core(params.url)
        .await
        .map(Json)
}

pub async fn preset_gallery_fetch_preset(
    Json(params): Json<PresetGalleryPresetParams>,
) -> Result<Json<GalleryDocument>, AppCommandError> {
    preset_gallery_commands::preset_gallery_fetch_preset_core(params.url, params.sha256)
        .await
        .map(Json)
}
