//! Filesystem-backed workspace background-image repository.
//!
//! A single user-selected image is stored at
//! `paths::codeg_backgrounds_root()/background.img`. The repository is
//! **decoupled from Tauri** so the same routines back the desktop and
//! standalone-server runtimes, mirroring `crate::pets` — simplified to one
//! image with no id/manifest, and with relaxed validation: any dimensions are
//! allowed and no alpha channel is required (backgrounds are opaque photos).

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use image::{ImageFormat, ImageReader};

use crate::app_error::AppCommandError;
use crate::models::background::BackgroundAsset;
use crate::paths::codeg_backgrounds_root;

/// Smallest plausible image payload; rejecting tiny inputs early avoids
/// decoding random files.
const MIN_BG_BYTES: usize = 64;
/// Cap raw uploads at 16 MiB. A reasonable background is well under this; the
/// cap is purely a guardrail (matches the pet spritesheet ceiling).
const MAX_BG_BYTES: usize = 16 * 1024 * 1024;
/// Upper bound on decoded pixel count, checked from the header *before* any
/// full decode, so a decompression bomb cannot force a huge allocation.
/// 40M px ≈ an 8K image (7680×4320 ≈ 33M), generous for a wallpaper.
const MAX_BG_PIXELS: u64 = 40_000_000;
/// Canonical on-disk filename. Extension-agnostic — the mime type is sniffed
/// from the magic bytes on read, so a single path round-trips PNG/JPEG/WebP.
const BACKGROUND_FILENAME: &str = "background.img";

fn background_path() -> PathBuf {
    codeg_backgrounds_root().join(BACKGROUND_FILENAME)
}

/// Verify the payload is a real, bounded image before it touches disk. Accepts
/// PNG / JPEG / WebP. Unlike the sprite validator, it imposes no fixed
/// dimensions and requires no alpha channel. Dimensions are read from the
/// header (not a full decode) so a hostile file's declared pixel count is
/// bounded before allocation.
pub fn validate_background(bytes: &[u8]) -> Result<(), AppCommandError> {
    if bytes.len() < MIN_BG_BYTES {
        return Err(AppCommandError::invalid_input(
            "Background image payload is too small to be valid.",
        ));
    }
    if bytes.len() > MAX_BG_BYTES {
        return Err(AppCommandError::invalid_input(format!(
            "Background image exceeds {} MiB cap.",
            MAX_BG_BYTES / (1024 * 1024)
        )));
    }

    let cursor = std::io::Cursor::new(bytes);
    let reader = ImageReader::new(cursor)
        .with_guessed_format()
        .map_err(|e| AppCommandError::invalid_input(format!("Cannot read image header: {e}")))?;
    let format = reader.format().ok_or_else(|| {
        AppCommandError::invalid_input("Background must be a PNG, JPEG or WebP image.")
    })?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return Err(AppCommandError::invalid_input(
            "Background must be a PNG, JPEG or WebP image.",
        ));
    }

    let (w, h) = reader.into_dimensions().map_err(|e| {
        AppCommandError::invalid_input(format!("Cannot read image dimensions: {e}"))
    })?;
    if (w as u64) * (h as u64) > MAX_BG_PIXELS {
        return Err(AppCommandError::invalid_input(format!(
            "Background image is too large ({w}×{h}); the maximum is {MAX_BG_PIXELS} pixels."
        )));
    }
    Ok(())
}

fn ensure_backgrounds_root() -> Result<PathBuf, AppCommandError> {
    let root = codeg_backgrounds_root();
    if !root.exists() {
        fs::create_dir_all(&root).map_err(AppCommandError::io)?;
    }
    Ok(root)
}

fn write_background_atomic(bytes: &[u8]) -> Result<(), AppCommandError> {
    let root = ensure_backgrounds_root()?;
    let final_path = root.join(BACKGROUND_FILENAME);
    let tmp_path = root.join(format!("{BACKGROUND_FILENAME}.tmp"));
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

/// Header sniff to recover the mime on read. `validate_background` already ran
/// on write, so the on-disk bytes are guaranteed PNG/JPEG/WebP; the PNG
/// fallback only matters for a truncated/edited file.
fn sniff_mime(bytes: &[u8]) -> &'static str {
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        return "image/png";
    }
    if bytes.len() >= 3 && &bytes[..3] == b"\xFF\xD8\xFF" {
        return "image/jpeg";
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "image/webp";
    }
    "image/png"
}

/// Decode → validate → atomically overwrite the single background file.
pub fn set_background(image_base64: &str) -> Result<(), AppCommandError> {
    let bytes = decode_base64_payload(image_base64)?;
    validate_background(&bytes)?;
    write_background_atomic(&bytes)
}

/// Read the stored background, or `Ok(None)` when none is set. A missing file
/// is the normal "no background" state, not an error.
pub fn read_background() -> Result<Option<BackgroundAsset>, AppCommandError> {
    let path = background_path();
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(BackgroundAsset {
            mime: sniff_mime(&bytes).to_string(),
            data_base64: BASE64.encode(&bytes),
        })),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(AppCommandError::io(err)),
    }
}

/// Remove the stored background. Idempotent — an already-absent file is success.
pub fn clear_background() -> Result<(), AppCommandError> {
    match fs::remove_file(background_path()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AppCommandError::io(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Filesystem-touching paths depend on the global `CODEG_HOME`/`CODEG_DATA_DIR`
    // env (shared, races under parallel tests), so — like `pets::tests` — we
    // exercise the pure validation surface here and cover disk I/O via manual
    // smoke tests.

    fn encode_png(w: u32, h: u32) -> Vec<u8> {
        let mut img = image::RgbaImage::new(w, h);
        for (i, p) in img.pixels_mut().enumerate() {
            let v = (i % 251) as u8;
            *p = image::Rgba([v, v, v, 255]);
        }
        let mut bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn validate_accepts_reasonable_png() {
        assert!(validate_background(&encode_png(256, 256)).is_ok());
    }

    #[test]
    fn validate_accepts_arbitrary_dimensions() {
        // A wide banner and a tall strip both pass — no fixed-geometry rule.
        assert!(validate_background(&encode_png(1920, 200)).is_ok());
        assert!(validate_background(&encode_png(200, 1920)).is_ok());
    }

    #[test]
    fn validate_rejects_too_small() {
        let err = validate_background(&[0u8; 10]).unwrap_err();
        assert!(err.message.to_lowercase().contains("too small"));
    }

    #[test]
    fn validate_rejects_non_image() {
        // > MIN bytes but not a decodable image.
        let err = validate_background(&vec![0x42u8; 4096]).unwrap_err();
        assert!(err.message.contains("PNG"), "got: {}", err.message);
    }

    #[test]
    fn sniff_mime_detects_png() {
        assert_eq!(sniff_mime(&encode_png(64, 64)), "image/png");
    }

    #[test]
    fn sniff_mime_detects_jpeg_and_webp_magic() {
        assert_eq!(sniff_mime(b"\xFF\xD8\xFF\xE0abcd"), "image/jpeg");
        assert_eq!(sniff_mime(b"RIFF\x00\x00\x00\x00WEBPvp8"), "image/webp");
    }
}
