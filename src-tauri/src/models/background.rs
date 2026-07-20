//! Workspace background-image data model.

use serde::Serialize;

/// Asset payload streamed to the renderer for the workspace background image.
/// Mirrors `PetSpriteAsset`: base64 bytes plus the sniffed mime type, so the
/// frontend can build a `data:`/blob URL without a second round-trip.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundAsset {
    pub mime: String,
    pub data_base64: String,
}
