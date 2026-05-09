//! Desktop-pet data model.
//!
//! Field shapes mirror the Codex `/pet` + `/hatch` format so a directory under
//! `~/.codex/pets/<id>/` can be copied verbatim into `~/.codeg/pets/<id>/` and
//! work without further translation.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Sprite-sheet geometry locked to the Codex format.
pub const SPRITE_SHEET_WIDTH: u32 = 1536;
pub const SPRITE_SHEET_HEIGHT: u32 = 1872;
#[allow(dead_code)]
pub const SPRITE_GRID_COLS: u32 = 8;
#[allow(dead_code)]
pub const SPRITE_GRID_ROWS: u32 = 9;
#[allow(dead_code)]
pub const SPRITE_FRAME_WIDTH: u32 = SPRITE_SHEET_WIDTH / SPRITE_GRID_COLS; // 192
#[allow(dead_code)]
pub const SPRITE_FRAME_HEIGHT: u32 = SPRITE_SHEET_HEIGHT / SPRITE_GRID_ROWS; // 208

/// Filename codex writes inside each pet directory. Stored as a relative path
/// in `pet.json::spritesheetPath`; we resolve it against the pet directory.
pub const SPRITESHEET_FILENAME: &str = "spritesheet.webp";
pub const PET_MANIFEST_FILENAME: &str = "pet.json";

/// Animation rows in the spritesheet, top-to-bottom, mirroring the
/// openai/skills `animation-rows.md` ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PetState {
    #[default]
    Idle,
    RunningRight,
    RunningLeft,
    Waving,
    Jumping,
    Failed,
    Waiting,
    Running,
    Review,
}

impl PetState {
    pub const fn row(self) -> u8 {
        match self {
            PetState::Idle => 0,
            PetState::RunningRight => 1,
            PetState::RunningLeft => 2,
            PetState::Waving => 3,
            PetState::Jumping => 4,
            PetState::Failed => 5,
            PetState::Waiting => 6,
            PetState::Running => 7,
            PetState::Review => 8,
        }
    }
}

/// Persisted manifest. Field names match Codex's `pet.json` so the file is
/// byte-compatible (modulo formatting whitespace).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetManifest {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Codex always writes `"spritesheet.webp"` here. We keep the field for
    /// round-trip compatibility but ignore the value when locating the asset
    /// (see `SPRITESHEET_FILENAME`).
    pub spritesheet_path: String,
}

/// Flattened summary returned to the frontend's pet list / picker.
/// `spritesheet_path` is an *absolute* filesystem path so the frontend can
/// pass it back to a `read_pet_spritesheet` command without re-resolving.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetSummary {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub spritesheet_path: PathBuf,
}

/// Full pet metadata + asset path, returned by `pet_get`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetDetail {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub spritesheet_path: PathBuf,
}

/// Asset payload streamed to the renderer. WebP is preferred; PNG is
/// returned as-is when the user supplies a PNG sheet.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetSpriteAsset {
    pub mime: String,
    pub data_base64: String,
}

/// Input payload for `pet_add`. The frontend reads the file with the Tauri
/// dialog/file API, base64-encodes the bytes, and submits the rest as
/// metadata. The backend re-validates everything.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewPetInput {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Raw bytes of the spritesheet, base64-encoded. May be PNG or WebP.
    pub spritesheet_base64: String,
}

/// Patch payload for `pet_update_meta`. Only mutable fields are exposed; the
/// `id` is a primary key and renaming = recreate.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetMetaPatch {
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
}

/// One importable Codex pet seen on disk under `~/.codex/pets/`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportablePet {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub source_path: PathBuf,
    /// True when an entry of the same id already exists in `~/.codeg/pets/`.
    pub already_imported: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCodexPetsRequest {
    /// Subset of ids returned by `pet_list_importable_codex`. Empty = all.
    #[serde(default)]
    pub ids: Vec<String>,
    /// When true, conflicting ids get an `-imported` suffix; otherwise the
    /// import for that id is skipped and reported in `skipped`.
    #[serde(default)]
    pub overwrite_with_suffix: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCodexPetsResult {
    pub imported_ids: Vec<String>,
    pub skipped: Vec<ImportSkipped>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSkipped {
    pub source_id: String,
    pub reason: String,
}

/// Persisted UI state for the pet feature. JSON-serialized into the
/// `app_metadata` KV table under `pet.config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetWindowConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub active_pet_id: Option<String>,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default = "default_scale")]
    pub scale: f64,
    #[serde(default = "default_always_on_top")]
    pub always_on_top: bool,
}

fn default_scale() -> f64 {
    1.0
}

fn default_always_on_top() -> bool {
    true
}

impl Default for PetWindowConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            active_pet_id: None,
            x: None,
            y: None,
            scale: 1.0,
            always_on_top: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetWindowStatePatch {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub scale: Option<f64>,
    pub always_on_top: Option<bool>,
    pub enabled: Option<bool>,
}

/// Internal hint emitted by the ACP/event subsystem and consumed by the
/// pet state mapper. Public so unit tests can construct hints directly.
/// Currently unused — `pet_state_mapper` consumes raw `AcpEvent`s and
/// derives state from the broadcast — but kept as a stable shape for
/// future Phase 2 refactoring (git-workflow signals, manual triggers).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PetStateHint {
    /// A connection began streaming output for an in-flight prompt.
    PromptStart,
    /// A streaming prompt completed (success).
    PromptEnd,
    /// A connection raised an error.
    Error,
    /// A connection asks the user to grant a permission.
    PermissionPending,
    /// A pending permission has been resolved.
    PermissionResolved,
    /// A turn ended in a state that wants user review (e.g. plan / diff).
    ReviewPending,
    /// Review consumed.
    ReviewResolved,
    /// A connection was added (new ACP session).
    ConnectionAdded,
    /// A connection was removed (disconnect / reap).
    ConnectionRemoved,
    /// Reset all transient flags to a clean Idle state. Used on startup.
    Reset,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_geometry_matches_codex() {
        assert_eq!(SPRITE_FRAME_WIDTH, 192);
        assert_eq!(SPRITE_FRAME_HEIGHT, 208);
    }

    #[test]
    fn pet_state_rows_are_unique_and_sequential() {
        let states = [
            PetState::Idle,
            PetState::RunningRight,
            PetState::RunningLeft,
            PetState::Waving,
            PetState::Jumping,
            PetState::Failed,
            PetState::Waiting,
            PetState::Running,
            PetState::Review,
        ];
        for (i, s) in states.iter().enumerate() {
            assert_eq!(s.row() as usize, i);
        }
    }

    #[test]
    fn manifest_round_trips_codex_layout() {
        // Verify the on-disk shape is byte-compatible with what codex writes.
        let raw = r#"{
            "id": "duck",
            "displayName": "Dewey",
            "description": "A small duck.",
            "spritesheetPath": "spritesheet.webp"
        }"#;
        let manifest: PetManifest = serde_json::from_str(raw).expect("parse codex pet.json");
        assert_eq!(manifest.id, "duck");
        assert_eq!(manifest.display_name, "Dewey");
        assert_eq!(manifest.spritesheet_path, "spritesheet.webp");

        let reserialized = serde_json::to_value(&manifest).unwrap();
        assert_eq!(reserialized["displayName"], "Dewey");
        assert_eq!(reserialized["spritesheetPath"], "spritesheet.webp");
    }
}
