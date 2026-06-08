//! Restore: stage-then-swap-on-startup.
//!
//! The DB connection pool holds `codeg.db` open (WAL sidecars), so swapping it
//! under a live connection risks corruption (and fails outright on Windows).
//! Restore therefore runs in two phases:
//!
//! 1. **Stage** (while running) — decrypt + extract + checksum-verify the
//!    archive into `<data_dir>/.codeg-restore-staging/<op_id>/`, then write a
//!    pending-restore marker. Live data is untouched until this fully succeeds.
//! 2. **Swap** (next startup) — [`apply_pending_restore_on_startup`] runs as the
//!    first step of `db::init_database`, before any connection is opened: it
//!    takes a safety snapshot of the current data, moves the staged files into
//!    place, then lets the normal `Migrator::up` bring a possibly-older
//!    restored DB up to the current schema.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::app_error::{AppCommandError, BACKUP_I18N_KEY_ALREADY_PENDING};

use super::archive;
use super::core;
use super::crypto;
use super::manifest::{BackupManifest, BackupPhase, BackupProgress, BACKUP_PROGRESS_EVENT};
use crate::web::event_bridge::{emit_event, EventEmitter};

/// Marker committing a staged restore; consumed on next startup.
pub const PENDING_MARKER: &str = ".codeg-restore-pending.json";
/// Root for staged (extracted, verified, not-yet-applied) restore payloads.
pub const STAGING_DIR: &str = ".codeg-restore-staging";
/// Root for pre-restore safety snapshots of the previous live data.
pub const SAFETY_DIR: &str = ".codeg-restore-backup";
/// Side location external transcripts are restored to (never clobbers the
/// live CLI dirs without explicit conflict resolution — see M7).
pub const RESTORED_TRANSCRIPTS_DIR: &str = "restored-transcripts";
/// Transient dir (server mode) holding export archives awaiting download.
pub const EXPORT_TMP_DIR: &str = ".codeg-backup-tmp";
/// Transient dir (server mode) holding uploaded archives awaiting inspect/stage.
pub const UPLOAD_TMP_DIR: &str = ".codeg-restore-upload";

/// How conflicting files are handled when restoring external transcripts back
/// to their original CLI locations. Never silent: the UI forces an explicit
/// choice before `Overwrite` can be selected.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    Overwrite,
    SkipExisting,
}

/// Where (if anywhere) external agent transcripts are restored.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ExternalRestoreMode {
    /// Don't restore external transcripts at all.
    #[default]
    Skip,
    /// Extract to a safe side folder under the data dir (zero risk; default).
    SideLocation,
    /// Write back to the original `~/.claude` etc., honoring `on_conflict`.
    OriginalLocations { on_conflict: ConflictPolicy },
}

/// Result of staging a restore (returned to the UI before the restart).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedRestore {
    pub staging_dir: String,
    pub manifest: BackupManifest,
    /// Set when external transcripts were extracted to a side location.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored_external_path: Option<String>,
    /// External files skipped because they already existed and the user did
    /// not authorize overwriting them.
    pub skipped_conflicts: Vec<String>,
}

/// Outcome of the startup swap.
#[derive(Debug)]
pub enum RestoreApplied {
    None,
    Applied { safety_snapshot: Option<PathBuf> },
}

#[derive(Debug, Serialize, Deserialize)]
struct PendingRestore {
    staging_dir: String,
    created_at: String,
    app_version: String,
    latest_migration: String,
}

/// Decrypt + extract + verify a backup into a staging dir and write the pending
/// marker. Does NOT touch live data — the swap happens at next startup.
pub(crate) async fn stage_restore_core(
    src: &Path,
    data_dir: &Path,
    passphrase: Option<&str>,
    external_mode: ExternalRestoreMode,
    emitter: &EventEmitter,
    op_id: &str,
    cancel: &CancellationToken,
) -> Result<StagedRestore, AppCommandError> {
    // 0. Only one restore may be staged at a time. Fail fast if one is already
    //    pending (the real guard is the atomic no-clobber marker write in step
    //    4; this just avoids wasting extraction work in the common case).
    if data_dir.join(PENDING_MARKER).exists() {
        return Err(already_pending_error());
    }

    // 1. Detect encryption + obtain a plaintext ZIP (decrypt-to-temp if needed;
    //    a wrong passphrase fails here via the GCM tag).
    let src_buf = src.to_path_buf();
    let encrypted = tokio::task::spawn_blocking(move || crypto::is_encrypted(&src_buf))
        .await
        .map_err(spawn_err)??;
    let (zip_path, _guard) = core::obtain_plaintext_zip(src, encrypted, passphrase).await?;

    // 2. Read + validate the manifest (version gate).
    let zip_for_manifest = zip_path.clone();
    let manifest = tokio::task::spawn_blocking(move || archive::read_manifest(&zip_for_manifest))
        .await
        .map_err(spawn_err)??;
    let (compatible, reject) = core::evaluate_compat(&manifest);
    if !compatible {
        return Err(reject_to_error(&manifest, reject.as_deref()));
    }
    // Reject crafted manifests (traversal/dup paths, missing DB) before we
    // trust the manifest to bound extraction.
    archive::validate_manifest(&manifest)?;

    // 3. Extract into a fresh staging dir + verify every checksum. Extraction
    //    is manifest-bounded, so the staged set equals the checksum-covered set.
    let staging_root = data_dir.join(STAGING_DIR).join(op_id);
    let _ = tokio::fs::remove_dir_all(&staging_root).await;
    tokio::fs::create_dir_all(&staging_root)
        .await
        .map_err(AppCommandError::io)?;

    emit(emitter, op_id, BackupPhase::Extracting);
    let zip_c = zip_path.clone();
    let staging_c = staging_root.clone();
    let manifest_c = manifest.clone();
    let cancel_c = cancel.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppCommandError> {
        archive::extract_all(&zip_c, &staging_c, &manifest_c, &cancel_c, &mut archive::null_progress())?;
        archive::verify_checksums(&staging_c, &manifest_c, &cancel_c)
    })
    .await
    .map_err(spawn_err)??;
    emit(emitter, op_id, BackupPhase::Verifying);

    // `uploads/` is an always-managed section: ensure the staged dir exists
    // even when the backup carried zero upload files, so the swap REPLACES live
    // uploads wholesale (a backup with empty uploads must not leave stale live
    // files behind). The DB is always present; tokens/preferences are
    // replaced only when the backup actually carried them (machine-bound
    // secrets are deliberately not wiped by a backup that never had them).
    tokio::fs::create_dir_all(staging_root.join("uploads"))
        .await
        .map_err(AppCommandError::io)?;

    // 4. Commit core restore by writing the pending marker FIRST. The marker is
    //    the point of no return for the DB/uploads swap (applied next startup).
    write_pending_marker(data_dir, &staging_root, &manifest)?;

    // 5. External transcripts run AFTER the commit and are TRULY non-fatal: the
    //    core restore is already committed, so an external (best-effort,
    //    non-transactional) write must never turn the call into an error — that
    //    would tell the UI "failed / don't restart" while the marker silently
    //    applies on the next launch. Any external failure is logged and the
    //    stage still reports success.
    let (restored_external_path, skipped_conflicts) =
        match handle_external(&staging_root, data_dir, &manifest, external_mode, cancel).await {
            Ok(result) => result,
            Err(e) => {
                eprintln!(
                    "[RESTORE] external transcript handling failed (core restore still staged): {e}"
                );
                (None, Vec::new())
            }
        };

    emit(emitter, op_id, BackupPhase::Done);

    Ok(StagedRestore {
        staging_dir: staging_root.to_string_lossy().into_owned(),
        manifest,
        restored_external_path,
        skipped_conflicts,
    })
}

/// Remove transient backup/restore scratch dirs left behind by an interrupted
/// process (export archives whose reaper never fired, uploads whose stage never
/// completed, and orphaned staging with no pending marker). Best-effort; called
/// at startup after [`apply_pending_restore_on_startup`]. No-op on desktop,
/// which uses neither transient dir.
pub fn cleanup_transient_dirs(data_dir: &Path) {
    let _ = std::fs::remove_dir_all(data_dir.join(EXPORT_TMP_DIR));
    let _ = std::fs::remove_dir_all(data_dir.join(UPLOAD_TMP_DIR));
    // A staging dir is only valid while its pending marker exists; once the
    // marker is gone (applied or never committed), staging is orphaned.
    if !data_dir.join(PENDING_MARKER).exists() {
        let _ = std::fs::remove_dir_all(data_dir.join(STAGING_DIR));
    }
}

/// Apply a staged restore if a pending marker exists. MUST run before the DB
/// connection is opened. Pure filesystem; crash-safe and idempotent.
///
/// Resolves the live uploads root + preferences path via the env-aware
/// `paths::*` resolvers (production), then delegates to
/// [`apply_pending_restore_with_paths`]. Tests call the inner fn with temp
/// paths so they never touch the real `~/.codeg`.
pub fn apply_pending_restore_on_startup(
    data_dir: &Path,
) -> Result<RestoreApplied, std::io::Error> {
    apply_pending_restore_with_paths(
        data_dir,
        &crate::paths::codeg_uploads_root(),
        &crate::paths::codeg_home_dir().join("preferences.json"),
    )
}

pub(crate) fn apply_pending_restore_with_paths(
    data_dir: &Path,
    uploads_root: &Path,
    preferences_path: &Path,
) -> Result<RestoreApplied, std::io::Error> {
    let marker = data_dir.join(PENDING_MARKER);
    if !marker.is_file() {
        return Ok(RestoreApplied::None);
    }
    let raw = std::fs::read_to_string(&marker)?;
    let pending: PendingRestore = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(_) => {
            // Corrupt / half-written marker — discard so we don't loop.
            eprintln!("[RESTORE] ignoring malformed pending-restore marker");
            let _ = std::fs::remove_file(&marker);
            return Ok(RestoreApplied::None);
        }
    };
    let staging = PathBuf::from(&pending.staging_dir);
    if !staging.is_dir() {
        eprintln!("[RESTORE] staging dir missing, discarding marker");
        let _ = std::fs::remove_file(&marker);
        return Ok(RestoreApplied::None);
    }

    eprintln!(
        "[RESTORE] applying staged restore (backup app_version={}, migration={})",
        pending.app_version, pending.latest_migration
    );

    // Safety snapshot of the current live data, then swap staged files in.
    let backup_dir = data_dir.join(SAFETY_DIR).join(safe_timestamp());
    std::fs::create_dir_all(&backup_dir)?;

    let db_name = crate::db::database_file_name();
    swap_in(
        &staging.join("db").join("codeg.db"),
        &data_dir.join(db_name),
        &backup_dir.join(db_name),
    )?;
    // Move any live WAL sidecars aside (no staged counterpart — the snapshot is
    // a single consistent file).
    for sc in ["-wal", "-shm"] {
        let live = data_dir.join(format!("{db_name}{sc}"));
        if live.exists() {
            let _ = move_path(&live, &backup_dir.join(format!("{db_name}{sc}")));
        }
    }

    let staged_uploads = staging.join("uploads");
    if staged_uploads.is_dir() {
        swap_in(&staged_uploads, uploads_root, &backup_dir.join("uploads"))?;
    }

    let staged_tokens = staging.join("tokens.json");
    if staged_tokens.is_file() {
        swap_in(
            &staged_tokens,
            &data_dir.join("tokens.json"),
            &backup_dir.join("tokens.json"),
        )?;
    }

    let staged_prefs = staging.join("preferences.json");
    if staged_prefs.is_file() {
        swap_in(&staged_prefs, preferences_path, &backup_dir.join("preferences.json"))?;
    }

    // Commit only after a fully successful swap. On a mid-swap crash we return
    // Err with marker + staging + safety snapshot all intact, so the next boot
    // retries (swap_in is idempotent) or the operator recovers manually.
    std::fs::remove_file(&marker)?;
    let _ = std::fs::remove_dir_all(&staging);
    eprintln!(
        "[RESTORE] restore applied; previous data preserved at {}",
        backup_dir.display()
    );
    Ok(RestoreApplied::Applied {
        safety_snapshot: Some(backup_dir),
    })
}

/// Move `live` → `backup` (if present), then `staged` → `live`. Idempotent: if
/// `staged` is already gone (a prior interrupted run moved it), this is a no-op.
fn swap_in(staged: &Path, live: &Path, backup: &Path) -> std::io::Result<()> {
    if !staged.exists() {
        return Ok(());
    }
    if live.exists() {
        move_path(live, backup)?;
    }
    move_path(staged, live)?;
    Ok(())
}

/// Rename `src` → `dst`, falling back to recursive copy + remove across
/// filesystem boundaries (CODEG_HOME / CODEG_DATA_DIR may differ).
fn move_path(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_recursive(src, dst)?;
    if src.is_dir() {
        std::fs::remove_dir_all(src)?;
    } else {
        std::fs::remove_file(src)?;
    }
    Ok(())
}

fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in walkdir::WalkDir::new(src).follow_links(false) {
            let entry = entry.map_err(std::io::Error::other)?;
            let rel = entry.path().strip_prefix(src).unwrap_or(entry.path());
            let target = dst.join(rel);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&target)?;
            } else if entry.file_type().is_file() {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(entry.path(), &target)?;
            }
        }
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

/// Apply external transcripts from the staging dir per `mode`. External files
/// are owned by the agent CLIs, so OriginalLocations never overwrites an
/// existing file unless the caller authorized it (`Overwrite`); skipped paths
/// are returned for the UI to report.
async fn handle_external(
    staging_root: &Path,
    data_dir: &Path,
    manifest: &BackupManifest,
    mode: ExternalRestoreMode,
    cancel: &CancellationToken,
) -> Result<(Option<String>, Vec<String>), AppCommandError> {
    let staged_external = staging_root.join("external");
    if !manifest.includes_external_transcripts || !staged_external.is_dir() {
        return Ok((None, Vec::new()));
    }
    match mode {
        ExternalRestoreMode::Skip => {
            let _ = tokio::fs::remove_dir_all(&staged_external).await;
            Ok((None, Vec::new()))
        }
        ExternalRestoreMode::SideLocation => {
            // Zero-risk: move the whole tree to a timestamped side folder under
            // the data dir; the user copies it back manually if desired.
            let stamp = sanitize_stamp(&manifest.created_at);
            let dest = data_dir.join(RESTORED_TRANSCRIPTS_DIR).join(stamp);
            let staged_c = staged_external.clone();
            let dest_c = dest.clone();
            tokio::task::spawn_blocking(move || move_path(&staged_c, &dest_c))
                .await
                .map_err(spawn_err)?
                .map_err(AppCommandError::io)?;
            Ok((Some(dest.to_string_lossy().into_owned()), Vec::new()))
        }
        ExternalRestoreMode::OriginalLocations { on_conflict } => {
            let staged_c = staged_external.clone();
            let cancel_c = cancel.clone();
            let skipped = tokio::task::spawn_blocking(move || {
                super::external::restore_external_from_staging(&staged_c, on_conflict, &cancel_c)
            })
            .await
            .map_err(spawn_err)??;
            let _ = tokio::fs::remove_dir_all(&staged_external).await;
            Ok((None, skipped))
        }
    }
}

fn write_pending_marker(
    data_dir: &Path,
    staging_root: &Path,
    manifest: &BackupManifest,
) -> Result<(), AppCommandError> {
    let pending = PendingRestore {
        staging_dir: staging_root.to_string_lossy().into_owned(),
        created_at: Utc::now().to_rfc3339(),
        app_version: manifest.app_version.clone(),
        latest_migration: manifest.latest_migration.clone(),
    };
    let json = serde_json::to_vec_pretty(&pending)
        .map_err(|e| AppCommandError::task_execution_failed("Serialize restore marker").with_detail(e.to_string()))?;
    let marker = data_dir.join(PENDING_MARKER);
    // Atomic, no-clobber claim: `create_new` lets exactly one concurrent stage
    // commit. A second one fails with AlreadyExists rather than racing a rename
    // and silently committing a different staging dir. A crash mid-write leaves
    // a partial marker, which `apply_pending_restore_*` treats as malformed and
    // discards (its staging is then reaped by `cleanup_transient_dirs`).
    let mut f = match OpenOptions::new().write(true).create_new(true).open(&marker) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(already_pending_error())
        }
        Err(e) => return Err(AppCommandError::io(e)),
    };
    f.write_all(&json).map_err(AppCommandError::io)?;
    Ok(())
}

fn already_pending_error() -> AppCommandError {
    AppCommandError::already_exists(
        "A restore is already staged; restart to apply it before staging another",
    )
    .with_i18n(BACKUP_I18N_KEY_ALREADY_PENDING, Default::default())
}

fn reject_to_error(manifest: &BackupManifest, reason: Option<&str>) -> AppCommandError {
    use crate::app_error::BACKUP_I18N_KEY_NEWER_VERSION;
    match reason {
        Some(k) if k == BACKUP_I18N_KEY_NEWER_VERSION => {
            super::newer_version_error(&manifest.app_version, env!("CARGO_PKG_VERSION"))
        }
        _ => super::unknown_format_error(),
    }
}

fn safe_timestamp() -> String {
    format!(
        "{}-{}",
        Utc::now().format("%Y%m%d-%H%M%S"),
        uuid::Uuid::new_v4().simple()
    )
}

fn sanitize_stamp(rfc3339: &str) -> String {
    rfc3339
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn emit(emitter: &EventEmitter, op_id: &str, phase: BackupPhase) {
    emit_event(emitter, BACKUP_PROGRESS_EVENT, BackupProgress::phase(op_id, phase));
}

fn spawn_err(e: tokio::task::JoinError) -> AppCommandError {
    AppCommandError::task_execution_failed("Restore task failed").with_detail(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_in_idempotent_and_takes_safety_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("staged.txt");
        let live = dir.path().join("live.txt");
        let backup = dir.path().join("backup/live.txt");
        std::fs::write(&staged, b"new").unwrap();
        std::fs::write(&live, b"old").unwrap();

        swap_in(&staged, &live, &backup).unwrap();
        assert_eq!(std::fs::read(&live).unwrap(), b"new");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old");
        assert!(!staged.exists());

        // Re-running with staged already gone is a no-op (idempotent).
        swap_in(&staged, &live, &backup).unwrap();
        assert_eq!(std::fs::read(&live).unwrap(), b"new");
    }

    #[test]
    fn apply_is_noop_without_marker() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            apply_pending_restore_on_startup(dir.path()).unwrap(),
            RestoreApplied::None
        ));
    }

    #[test]
    fn apply_ignores_malformed_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(PENDING_MARKER), b"{not json").unwrap();
        assert!(matches!(
            apply_pending_restore_on_startup(dir.path()).unwrap(),
            RestoreApplied::None
        ));
        assert!(!dir.path().join(PENDING_MARKER).exists());
    }

    #[test]
    fn apply_swaps_staged_db_and_snapshots_old() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();
        let db_name = crate::db::database_file_name();

        // Current live DB + a staged replacement.
        std::fs::write(data_dir.join(db_name), b"OLD-DB").unwrap();
        let staging = data_dir.join(STAGING_DIR).join("op1");
        std::fs::create_dir_all(staging.join("db")).unwrap();
        std::fs::write(staging.join("db").join("codeg.db"), b"NEW-DB").unwrap();

        let marker = PendingRestore {
            staging_dir: staging.to_string_lossy().into_owned(),
            created_at: "2026-06-06T00:00:00Z".to_string(),
            app_version: "0.15.0".to_string(),
            latest_migration: "m20260522_000001_delegation_columns".to_string(),
        };
        std::fs::write(
            data_dir.join(PENDING_MARKER),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();

        let applied = apply_pending_restore_on_startup(data_dir).unwrap();
        match applied {
            RestoreApplied::Applied { safety_snapshot } => {
                let snap = safety_snapshot.expect("snapshot");
                assert_eq!(std::fs::read(snap.join(db_name)).unwrap(), b"OLD-DB");
            }
            RestoreApplied::None => panic!("expected a restore to be applied"),
        }
        assert_eq!(std::fs::read(data_dir.join(db_name)).unwrap(), b"NEW-DB");
        assert!(!data_dir.join(PENDING_MARKER).exists());
        assert!(!staging.exists());

        // Idempotent second call: nothing pending.
        assert!(matches!(
            apply_pending_restore_on_startup(data_dir).unwrap(),
            RestoreApplied::None
        ));
    }
}
