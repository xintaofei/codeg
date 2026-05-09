//! Filesystem-backed pet repository.
//!
//! All access is synchronous I/O wrapped in `tokio::task::spawn_blocking` by
//! callers when needed. The repository reads from / writes to
//! `paths::codeg_pets_root()` and is **decoupled from Tauri** so the same
//! routines back the desktop and standalone-server runtimes.
//!
//! Format mirrors Codex `/pet`:
//!
//! ```text
//! <pets-root>/<pet-id>/
//!     pet.json
//!     spritesheet.webp
//! ```
//!
//! Where `pet.json` carries `{ id, displayName, description?, spritesheetPath }`
//! and the spritesheet is a 1536×1872 RGBA WebP (PNG also accepted).

pub mod codex_import;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use image::{ImageFormat, ImageReader};

use crate::app_error::AppCommandError;
use crate::models::pet::{
    NewPetInput, PetDetail, PetManifest, PetMetaPatch, PetSpriteAsset, PetSummary,
    PET_MANIFEST_FILENAME, SPRITESHEET_FILENAME, SPRITE_SHEET_HEIGHT, SPRITE_SHEET_WIDTH,
};
use crate::paths::codeg_pets_root;

/// Smallest plausible sprite-sheet payload; rejecting tiny inputs early
/// avoids decoding random files.
const MIN_SPRITE_BYTES: usize = 1024;
/// Cap raw sprite uploads at 16 MiB. A correctly-encoded 1536×1872 WebP is
/// usually well under 1 MiB; this is purely a guardrail.
const MAX_SPRITE_BYTES: usize = 16 * 1024 * 1024;

/// Detected sprite-sheet container.
#[derive(Debug, Clone, Copy)]
pub enum SpriteFormat {
    Png,
    Webp,
}

impl SpriteFormat {
    pub const fn mime(self) -> &'static str {
        match self {
            SpriteFormat::Png => "image/png",
            SpriteFormat::Webp => "image/webp",
        }
    }

    pub const fn filename(self) -> &'static str {
        // We always *store* under the canonical Codex name so directories are
        // round-trippable. PNG uploads are renamed at write time.
        SPRITESHEET_FILENAME
    }
}

/// Decode the image header just enough to verify the sprite-sheet contract.
/// Returns the detected format on success.
pub fn validate_spritesheet(bytes: &[u8]) -> Result<SpriteFormat, AppCommandError> {
    if bytes.len() < MIN_SPRITE_BYTES {
        return Err(AppCommandError::invalid_input(
            "Spritesheet payload is too small to be valid.",
        ));
    }
    if bytes.len() > MAX_SPRITE_BYTES {
        return Err(AppCommandError::invalid_input(format!(
            "Spritesheet payload exceeds {} MiB cap.",
            MAX_SPRITE_BYTES / (1024 * 1024)
        )));
    }

    let cursor = std::io::Cursor::new(bytes);
    let reader = ImageReader::new(cursor)
        .with_guessed_format()
        .map_err(|e| AppCommandError::invalid_input(format!("Cannot read sprite header: {e}")))?;
    let format = reader.format().ok_or_else(|| {
        AppCommandError::invalid_input("Spritesheet must be a PNG or WebP image.")
    })?;
    let detected = match format {
        ImageFormat::Png => SpriteFormat::Png,
        ImageFormat::WebP => SpriteFormat::Webp,
        _ => {
            return Err(AppCommandError::invalid_input(
                "Spritesheet must be a PNG or WebP image.",
            ));
        }
    };

    let img = reader
        .decode()
        .map_err(|e| AppCommandError::invalid_input(format!("Cannot decode sprite: {e}")))?;
    let (w, h) = (img.width(), img.height());
    if w != SPRITE_SHEET_WIDTH || h != SPRITE_SHEET_HEIGHT {
        return Err(AppCommandError::invalid_input(format!(
            "Spritesheet must be {SPRITE_SHEET_WIDTH}x{SPRITE_SHEET_HEIGHT} pixels (got {w}x{h})."
        )));
    }
    if !img.color().has_alpha() {
        return Err(AppCommandError::invalid_input(
            "Spritesheet must contain an alpha channel (transparent background).",
        ));
    }

    Ok(detected)
}

/// Slug-validate a pet id. Returns `Err` on invalid input. Defense in depth:
/// the frontend slugifies but we must independently reject malformed ids
/// before they touch the filesystem.
pub fn validate_pet_id(id: &str) -> Result<(), AppCommandError> {
    if id.is_empty() {
        return Err(AppCommandError::invalid_input("Pet id is required."));
    }
    if id.len() > 64 {
        return Err(AppCommandError::invalid_input(
            "Pet id must be at most 64 characters.",
        ));
    }
    let valid = id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !valid {
        return Err(AppCommandError::invalid_input(
            "Pet id may only contain lowercase letters, digits, '-' and '_'.",
        ));
    }
    if id.starts_with('-') || id.ends_with('-') || id.starts_with('.') {
        return Err(AppCommandError::invalid_input(
            "Pet id cannot start with '.' or '-' / end with '-'.",
        ));
    }
    Ok(())
}

fn pet_dir(id: &str) -> Result<PathBuf, AppCommandError> {
    validate_pet_id(id)?;
    Ok(codeg_pets_root().join(id))
}

fn ensure_pets_root() -> Result<PathBuf, AppCommandError> {
    ensure_pets_root_or_create()
}

/// Public alias used by `codex_import` to share the same create-if-missing
/// behaviour without exposing the rest of the module's private helpers.
pub(crate) fn ensure_pets_root_or_create() -> Result<PathBuf, AppCommandError> {
    let root = codeg_pets_root();
    if !root.exists() {
        fs::create_dir_all(&root).map_err(AppCommandError::io)?;
    }
    Ok(root)
}

/// Snapshot of pet ids currently on disk. Exposed for `codex_import` to
/// detect collisions before copying.
pub(crate) fn list_existing_ids() -> Result<std::collections::HashSet<String>, AppCommandError> {
    let root = codeg_pets_root();
    if !root.is_dir() {
        return Ok(std::collections::HashSet::new());
    }
    let mut out = std::collections::HashSet::new();
    for entry in fs::read_dir(&root).map_err(AppCommandError::io)?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            out.insert(name.to_string());
        }
    }
    Ok(out)
}

fn read_manifest(dir: &Path) -> Result<PetManifest, AppCommandError> {
    let manifest_path = dir.join(PET_MANIFEST_FILENAME);
    let raw = fs::read_to_string(&manifest_path).map_err(AppCommandError::io)?;
    serde_json::from_str::<PetManifest>(&raw).map_err(|e| {
        AppCommandError::invalid_input(format!(
            "Malformed pet manifest at {}: {e}",
            manifest_path.display()
        ))
    })
}

fn write_manifest_atomic(dir: &Path, manifest: &PetManifest) -> Result<(), AppCommandError> {
    let final_path = dir.join(PET_MANIFEST_FILENAME);
    let tmp_path = dir.join(format!("{PET_MANIFEST_FILENAME}.tmp"));
    let json = serde_json::to_string_pretty(manifest).map_err(|e| {
        AppCommandError::io_error(format!("Failed to serialize pet manifest: {e}"))
    })?;
    {
        let mut f = fs::File::create(&tmp_path).map_err(AppCommandError::io)?;
        f.write_all(json.as_bytes()).map_err(AppCommandError::io)?;
        f.write_all(b"\n").map_err(AppCommandError::io)?;
        f.sync_all().map_err(AppCommandError::io)?;
    }
    fs::rename(&tmp_path, &final_path).map_err(AppCommandError::io)?;
    Ok(())
}

fn write_spritesheet_atomic(dir: &Path, bytes: &[u8]) -> Result<(), AppCommandError> {
    let final_path = dir.join(SPRITESHEET_FILENAME);
    let tmp_path = dir.join(format!("{SPRITESHEET_FILENAME}.tmp"));
    {
        let mut f = fs::File::create(&tmp_path).map_err(AppCommandError::io)?;
        f.write_all(bytes).map_err(AppCommandError::io)?;
        f.sync_all().map_err(AppCommandError::io)?;
    }
    fs::rename(&tmp_path, &final_path).map_err(AppCommandError::io)?;
    Ok(())
}

fn decode_base64_payload(b64: &str) -> Result<Vec<u8>, AppCommandError> {
    BASE64
        .decode(b64.as_bytes())
        .map_err(|e| AppCommandError::invalid_input(format!("Invalid base64 payload: {e}")))
}

/// Enumerate well-formed pets in `pets-root`. Bad entries (missing files,
/// malformed manifests) are skipped silently so a single corrupt directory
/// cannot break the picker. The bad entries get logged for diagnosis.
pub fn list_pets() -> Result<Vec<PetSummary>, AppCommandError> {
    let root = codeg_pets_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let entries = match fs::read_dir(&root) {
        Ok(it) => it,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(AppCommandError::io(err)),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest = match read_manifest(&path) {
            Ok(m) => m,
            Err(err) => {
                eprintln!(
                    "[Pets] skipping {}: {}",
                    path.display(),
                    err.message
                );
                continue;
            }
        };
        let spritesheet = path.join(SPRITESHEET_FILENAME);
        if !spritesheet.exists() {
            eprintln!(
                "[Pets] skipping {}: spritesheet missing",
                path.display()
            );
            continue;
        }
        out.push(PetSummary {
            id: manifest.id,
            display_name: manifest.display_name,
            description: manifest.description,
            spritesheet_path: spritesheet,
        });
    }
    out.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
    Ok(out)
}

pub fn get_pet(id: &str) -> Result<PetDetail, AppCommandError> {
    let dir = pet_dir(id)?;
    if !dir.is_dir() {
        return Err(AppCommandError::not_found(format!("Pet '{id}' not found.")));
    }
    let manifest = read_manifest(&dir)?;
    let spritesheet = dir.join(SPRITESHEET_FILENAME);
    if !spritesheet.exists() {
        return Err(AppCommandError::not_found(format!(
            "Pet '{id}' has no spritesheet on disk."
        )));
    }
    Ok(PetDetail {
        id: manifest.id,
        display_name: manifest.display_name,
        description: manifest.description,
        spritesheet_path: spritesheet,
    })
}

pub fn read_pet_spritesheet(id: &str) -> Result<PetSpriteAsset, AppCommandError> {
    let dir = pet_dir(id)?;
    let spritesheet = dir.join(SPRITESHEET_FILENAME);
    let bytes = fs::read(&spritesheet).map_err(AppCommandError::io)?;
    let mime = sniff_mime(&bytes);
    Ok(PetSpriteAsset {
        mime: mime.to_string(),
        data_base64: BASE64.encode(&bytes),
    })
}

fn sniff_mime(bytes: &[u8]) -> &'static str {
    // Header sniff is enough — `validate_spritesheet` already ran on write,
    // so on-disk bytes are guaranteed PNG or WebP. Fallback to webp on
    // ambiguity since that's the canonical Codex format.
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        return "image/png";
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "image/webp";
    }
    "image/webp"
}

pub fn add_pet(input: NewPetInput) -> Result<PetSummary, AppCommandError> {
    validate_pet_id(&input.id)?;
    if input.display_name.trim().is_empty() {
        return Err(AppCommandError::invalid_input("Display name is required."));
    }

    let bytes = decode_base64_payload(&input.spritesheet_base64)?;
    let _format = validate_spritesheet(&bytes)?;

    let root = ensure_pets_root()?;
    let target = root.join(&input.id);
    if target.exists() {
        return Err(AppCommandError::already_exists(format!(
            "A pet with id '{}' already exists.",
            input.id
        )));
    }

    // Stage in a sibling tmp dir, then rename atomically so a crashed
    // mid-write never leaves a half-built pet on disk.
    let tmp_dir = root.join(format!("{}.import.tmp", input.id));
    if tmp_dir.exists() {
        // Leftover from a previous failure — purge it.
        let _ = fs::remove_dir_all(&tmp_dir);
    }
    fs::create_dir_all(&tmp_dir).map_err(AppCommandError::io)?;

    let manifest = PetManifest {
        id: input.id.clone(),
        display_name: input.display_name.trim().to_string(),
        description: input
            .description
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        spritesheet_path: SPRITESHEET_FILENAME.to_string(),
    };
    if let Err(err) = write_manifest_atomic(&tmp_dir, &manifest) {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(err);
    }
    if let Err(err) = write_spritesheet_atomic(&tmp_dir, &bytes) {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(err);
    }

    if let Err(err) = fs::rename(&tmp_dir, &target) {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(AppCommandError::io(err));
    }

    Ok(PetSummary {
        id: manifest.id,
        display_name: manifest.display_name,
        description: manifest.description,
        spritesheet_path: target.join(SPRITESHEET_FILENAME),
    })
}

pub fn update_pet_meta(id: &str, patch: PetMetaPatch) -> Result<PetSummary, AppCommandError> {
    let dir = pet_dir(id)?;
    if !dir.is_dir() {
        return Err(AppCommandError::not_found(format!("Pet '{id}' not found.")));
    }
    let mut manifest = read_manifest(&dir)?;

    if let Some(name) = patch.display_name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(AppCommandError::invalid_input(
                "Display name cannot be blank.",
            ));
        }
        manifest.display_name = trimmed.to_string();
    }
    if let Some(desc) = patch.description {
        manifest.description = desc.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    }

    write_manifest_atomic(&dir, &manifest)?;
    Ok(PetSummary {
        id: manifest.id,
        display_name: manifest.display_name,
        description: manifest.description,
        spritesheet_path: dir.join(SPRITESHEET_FILENAME),
    })
}

pub fn replace_pet_sprite(id: &str, spritesheet_base64: &str) -> Result<(), AppCommandError> {
    let dir = pet_dir(id)?;
    if !dir.is_dir() {
        return Err(AppCommandError::not_found(format!("Pet '{id}' not found.")));
    }
    let bytes = decode_base64_payload(spritesheet_base64)?;
    validate_spritesheet(&bytes)?;
    write_spritesheet_atomic(&dir, &bytes)
}

pub fn delete_pet(id: &str) -> Result<(), AppCommandError> {
    let dir = pet_dir(id)?;
    if !dir.is_dir() {
        // Idempotent delete — already gone is success.
        return Ok(());
    }
    fs::remove_dir_all(&dir).map_err(AppCommandError::io)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Filesystem-touching tests rely on `CODEG_HOME`, which is shared global
    // state. Cargo runs tests in parallel, so we'd need cross-binary
    // serialization (e.g. `serial_test`) to make them reliable. Instead, we
    // exercise the validation surface directly here — the disk path is
    // covered end-to-end by the manual smoke tests in
    // `.docs/dev-design/2026-05-08-桌面宠物.md`.

    #[test]
    fn validate_pet_id_accepts_valid() {
        assert!(validate_pet_id("duck").is_ok());
        assert!(validate_pet_id("dewey-the-duck").is_ok());
        assert!(validate_pet_id("pet_42").is_ok());
        assert!(validate_pet_id("a").is_ok());
    }

    #[test]
    fn validate_pet_id_rejects_invalid() {
        assert!(validate_pet_id("").is_err());
        assert!(validate_pet_id("Bad/Id").is_err());
        assert!(validate_pet_id("UPPER").is_err());
        assert!(validate_pet_id("-leading").is_err());
        assert!(validate_pet_id("trailing-").is_err());
        assert!(validate_pet_id(".dotfile").is_err());
        assert!(validate_pet_id(&"a".repeat(65)).is_err());
        assert!(validate_pet_id("space here").is_err());
    }

    #[test]
    fn validate_spritesheet_rejects_too_small_payload() {
        let bytes = vec![0u8; 100];
        let err = validate_spritesheet(&bytes).unwrap_err();
        assert!(err.message.to_lowercase().contains("too small"));
    }

    #[test]
    fn validate_spritesheet_rejects_wrong_dimensions() {
        // Encode a 100x100 RGBA PNG, then pad with non-meaningful bytes to
        // bypass the size guard. The decode step still rejects the
        // dimensions.
        let img = image::RgbaImage::from_pixel(100, 100, image::Rgba([0, 0, 0, 255]));
        let mut bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        // Front-pad nothing; PNG can't be appended-to. Just ensure size > MIN.
        // For most encoder builds a solid 100x100 PNG is < MIN_SPRITE_BYTES,
        // so we ladder up to 200x200 to cross the threshold while still
        // failing the dimension check.
        let img2 = image::RgbaImage::from_pixel(200, 200, image::Rgba([10, 20, 30, 255]));
        let mut bytes2: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img2)
            .write_to(&mut std::io::Cursor::new(&mut bytes2), ImageFormat::Png)
            .unwrap();
        let candidate = if bytes2.len() >= MIN_SPRITE_BYTES {
            bytes2
        } else {
            // Encode a noisy 1024x1024 to guarantee size > MIN.
            let mut img3 = image::RgbaImage::new(1024, 1024);
            for (i, p) in img3.pixels_mut().enumerate() {
                let v = (i % 251) as u8;
                *p = image::Rgba([v, v, v, 255]);
            }
            let mut b: Vec<u8> = Vec::new();
            image::DynamicImage::ImageRgba8(img3)
                .write_to(&mut std::io::Cursor::new(&mut b), ImageFormat::Png)
                .unwrap();
            b
        };
        let err = validate_spritesheet(&candidate).unwrap_err();
        assert!(
            err.message.contains("1536"),
            "expected dimension complaint, got: {}",
            err.message
        );
    }

    #[test]
    fn validate_spritesheet_accepts_correct_image() {
        let mut img =
            image::RgbaImage::new(SPRITE_SHEET_WIDTH, SPRITE_SHEET_HEIGHT);
        // Random-ish pattern to ensure we clear MIN_SPRITE_BYTES even after
        // PNG compresses it. Writing all-transparent zeroes would otherwise
        // shrink to a tiny payload.
        for (i, p) in img.pixels_mut().enumerate() {
            let v = (i % 251) as u8;
            *p = image::Rgba([v, v, v, v]);
        }
        let mut bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        let format = validate_spritesheet(&bytes).unwrap();
        assert!(matches!(format, SpriteFormat::Png));
    }
}
