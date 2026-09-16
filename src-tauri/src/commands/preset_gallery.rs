//! Tauri + Axum command surface for the appearance preset gallery.
//!
//! Thin double-mode wrappers over `crate::preset_gallery`, which owns the URL
//! policy, the byte caps and the digest check. Stateless (no DB / `AppState`),
//! like the `background_market_*` trio in `commands::background`.

use crate::app_error::AppCommandError;
use crate::preset_gallery::{self, GalleryDocument};

// ─── core ops ───────────────────────────────────────────────────────────

pub async fn preset_gallery_fetch_index_core(
    url: String,
) -> Result<GalleryDocument, AppCommandError> {
    preset_gallery::fetch_index(&url).await
}

pub async fn preset_gallery_fetch_preset_core(
    url: String,
    sha256: String,
) -> Result<GalleryDocument, AppCommandError> {
    preset_gallery::fetch_preset(&url, &sha256).await
}

// ─── web-handler param structs ──────────────────────────────────────────

/// Web-mode JSON bodies for the `preset_gallery_*` commands. The Tauri commands
/// take flat scalars (auto snake_case-translated on the way in); the Axum
/// handlers need named structs to deserialize the same camelCase payload.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetGalleryIndexParams {
    pub url: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetGalleryPresetParams {
    pub url: String,
    pub sha256: String,
}

// ─── tauri command wrappers ─────────────────────────────────────────────

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn preset_gallery_fetch_index(url: String) -> Result<GalleryDocument, AppCommandError> {
    preset_gallery_fetch_index_core(url).await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn preset_gallery_fetch_preset(
    url: String,
    sha256: String,
) -> Result<GalleryDocument, AppCommandError> {
    preset_gallery_fetch_preset_core(url, sha256).await
}
