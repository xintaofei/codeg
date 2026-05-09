//! One-way import: Codex pets → codeg pets.
//!
//! Codex stores pets at `${CODEX_HOME:-~/.codex}/pets/<id>/` with the same
//! `pet.json` + `spritesheet.webp` layout codeg uses. We copy the directory
//! tree verbatim so users can move a `/hatch`-ed pet over without losing
//! anything. We never write back to the Codex tree.

use std::fs;
use std::path::{Path, PathBuf};

use crate::app_error::AppCommandError;
use crate::models::pet::{
    ImportCodexPetsRequest, ImportCodexPetsResult, ImportSkipped, ImportablePet, PetManifest,
    PET_MANIFEST_FILENAME, SPRITESHEET_FILENAME,
};
use crate::pets::{ensure_pets_root_or_create, list_existing_ids, validate_pet_id};

/// Resolve `~/.codex/` honouring `$CODEX_HOME`. This duplicates the logic in
/// `parsers::codex` deliberately — keeping the module boundaries intact.
pub(crate) fn resolve_codex_home_dir() -> PathBuf {
    if let Some(custom) = std::env::var_os("CODEX_HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(custom);
    }
    dirs::home_dir().map(|h| h.join(".codex")).unwrap_or_default()
}

fn codex_pets_dir() -> PathBuf {
    resolve_codex_home_dir().join("pets")
}

/// List Codex pets that are eligible for import.
///
/// "Eligible" = manifest parses cleanly + `spritesheet.webp` exists. Bad
/// directories are filtered out so the importer UI never lists garbage.
pub fn list_importable_codex_pets() -> Result<Vec<ImportablePet>, AppCommandError> {
    let dir = codex_pets_dir();
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let existing = list_existing_ids().unwrap_or_default();

    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).map_err(AppCommandError::io)?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join(PET_MANIFEST_FILENAME);
        let raw = match fs::read_to_string(&manifest_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let manifest: PetManifest = match serde_json::from_str(&raw) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !path.join(SPRITESHEET_FILENAME).exists() {
            continue;
        }
        if validate_pet_id(&manifest.id).is_err() {
            // Codex slugs are usually fine, but defend anyway — we'll need
            // to write the id back to disk.
            continue;
        }
        let already = existing.contains(&manifest.id);
        out.push(ImportablePet {
            id: manifest.id,
            display_name: manifest.display_name,
            description: manifest.description,
            source_path: path,
            already_imported: already,
        });
    }
    out.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
    Ok(out)
}

/// Copy selected Codex pets into `~/.codeg/pets/`. Each entry copies the full
/// source directory; we deliberately re-validate nothing — Codex already
/// produced a working pet, and we want round-trip fidelity.
pub fn import_codex_pets(
    request: ImportCodexPetsRequest,
) -> Result<ImportCodexPetsResult, AppCommandError> {
    let target_root = ensure_pets_root_or_create()?;
    let candidates = list_importable_codex_pets()?;
    let wanted: Option<std::collections::HashSet<&str>> = if request.ids.is_empty() {
        None
    } else {
        Some(request.ids.iter().map(String::as_str).collect())
    };

    let mut imported = Vec::new();
    let mut skipped: Vec<ImportSkipped> = Vec::new();

    for pet in candidates {
        if let Some(filter) = wanted.as_ref() {
            if !filter.contains(pet.id.as_str()) {
                continue;
            }
        }
        let target_id = if pet.already_imported {
            if !request.overwrite_with_suffix {
                skipped.push(ImportSkipped {
                    source_id: pet.id.clone(),
                    reason: "Pet with this id already exists in codeg.".to_string(),
                });
                continue;
            }
            unique_id_with_suffix(&target_root, &pet.id)
        } else {
            pet.id.clone()
        };

        let target_dir = target_root.join(&target_id);
        if let Err(err) = copy_pet_dir(&pet.source_path, &target_dir, &target_id) {
            skipped.push(ImportSkipped {
                source_id: pet.id.clone(),
                reason: err.message,
            });
            continue;
        }
        imported.push(target_id);
    }

    Ok(ImportCodexPetsResult {
        imported_ids: imported,
        skipped,
    })
}

fn unique_id_with_suffix(target_root: &Path, base_id: &str) -> String {
    let mut candidate = format!("{base_id}-imported");
    let mut n = 2;
    while target_root.join(&candidate).exists() {
        candidate = format!("{base_id}-imported-{n}");
        n += 1;
    }
    candidate
}

fn copy_pet_dir(src: &Path, dst: &Path, target_id: &str) -> Result<(), AppCommandError> {
    if dst.exists() {
        return Err(AppCommandError::already_exists(format!(
            "Target directory {} already exists.",
            dst.display()
        )));
    }
    let parent = dst.parent().ok_or_else(|| {
        AppCommandError::io_error(format!("Cannot resolve parent of {}", dst.display()))
    })?;

    let tmp = parent.join(format!("{target_id}.import.tmp"));
    if tmp.exists() {
        let _ = fs::remove_dir_all(&tmp);
    }
    fs::create_dir_all(&tmp).map_err(AppCommandError::io)?;

    if let Err(err) = copy_dir_contents(src, &tmp, target_id) {
        let _ = fs::remove_dir_all(&tmp);
        return Err(err);
    }

    fs::rename(&tmp, dst).map_err(|err| {
        let _ = fs::remove_dir_all(&tmp);
        AppCommandError::io(err)
    })?;
    Ok(())
}

/// Copy `pet.json` and `spritesheet.webp` only. Other files in the source
/// directory are ignored — keeps the import surface deterministic and
/// avoids accidentally pulling in editor cruft.
fn copy_dir_contents(src: &Path, dst: &Path, target_id: &str) -> Result<(), AppCommandError> {
    // Manifest: rewrite `id` to the target id (which may have an
    // `-imported` suffix) so the pet is consistent with its directory name.
    let manifest_path = src.join(PET_MANIFEST_FILENAME);
    let raw = fs::read_to_string(&manifest_path).map_err(AppCommandError::io)?;
    let mut manifest: PetManifest = serde_json::from_str(&raw).map_err(|e| {
        AppCommandError::invalid_input(format!(
            "Source manifest at {} is malformed: {e}",
            manifest_path.display()
        ))
    })?;
    manifest.id = target_id.to_string();
    manifest.spritesheet_path = SPRITESHEET_FILENAME.to_string();

    let serialized = serde_json::to_string_pretty(&manifest)
        .map_err(|e| AppCommandError::io_error(format!("Cannot serialize manifest: {e}")))?;
    fs::write(dst.join(PET_MANIFEST_FILENAME), format!("{serialized}\n"))
        .map_err(AppCommandError::io)?;

    let src_sheet = src.join(SPRITESHEET_FILENAME);
    if !src_sheet.exists() {
        return Err(AppCommandError::not_found(format!(
            "Source spritesheet missing at {}",
            src_sheet.display()
        )));
    }
    fs::copy(&src_sheet, dst.join(SPRITESHEET_FILENAME)).map_err(AppCommandError::io)?;
    Ok(())
}

/// Whether the Codex pets directory exists at all. Lets the importer UI
/// disable its entry button cleanly when codex isn't installed.
pub fn codex_import_available() -> bool {
    codex_pets_dir().is_dir()
}

/// Convenience for callers that want the source path to display in UI.
pub fn codex_pets_root_for_display() -> PathBuf {
    codex_pets_dir()
}

