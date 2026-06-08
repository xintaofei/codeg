//! Runtime-agnostic backup engine.
//!
//! `create_backup_core` / `inspect_backup_core` take plain references
//! (`&DatabaseConnection`, `&EventEmitter`, `&CancellationToken`) so the same
//! code path serves the desktop Tauri commands, the Axum web handlers, and a
//! future headless scheduler (which would pass `EventEmitter::Noop`).

use std::path::{Path, PathBuf};

use chrono::Utc;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;
use tokio_util::sync::CancellationToken;

use crate::app_error::{AppCommandError, BACKUP_I18N_KEY_NEWER_VERSION, BACKUP_I18N_KEY_UNKNOWN_FORMAT};
use crate::db::migration::Migrator;
use crate::web::event_bridge::{emit_event, EventEmitter};

use super::archive::{self, ArchiveBuilder};
use super::crypto;
use super::external;
use super::manifest::{
    BackupManifest, BackupPhase, BackupPreview, BackupProgress, BACKUP_FORMAT_VERSION, BACKUP_KIND,
    BACKUP_PROGRESS_EVENT,
};
use super::cancelled_error;

/// Options that shape a backup.
#[derive(Debug, Clone, Default)]
pub struct BackupOptions {
    pub include_external_transcripts: bool,
    /// `None` or empty → unencrypted archive. Otherwise the archive is wrapped
    /// in an AES-256-GCM envelope keyed off this passphrase.
    pub passphrase: Option<String>,
}

/// Everything the engine needs to assemble a backup, resolved by the caller
/// (desktop command / web handler) so the engine stays free of env lookups.
pub struct BackupInputs<'a> {
    pub conn: &'a DatabaseConnection,
    pub data_dir: &'a Path,
    pub uploads_root: PathBuf,
    pub app_version: &'a str,
    pub runtime_label: &'static str,
}

/// Build a backup archive at `dest_path`. Emits [`BACKUP_PROGRESS_EVENT`]
/// throughout and honors `cancel`. Writes to a sibling `.part` file and renames
/// on success so a crash never leaves a half-written backup at `dest_path`.
pub(crate) async fn create_backup_core(
    inputs: BackupInputs<'_>,
    options: BackupOptions,
    dest_path: &Path,
    emitter: &EventEmitter,
    op_id: &str,
    cancel: &CancellationToken,
) -> Result<BackupManifest, AppCommandError> {
    let work = tempfile::tempdir().map_err(AppCommandError::io)?;
    let db_snapshot = work.path().join("codeg.db");
    let zip_tmp = work.path().join("payload.zip");

    // ── Phase 1: consistent DB snapshot via VACUUM INTO ──────────────────
    emit(emitter, op_id, BackupPhase::Snapshotting, 0, None, None);
    if cancel.is_cancelled() {
        return Err(cancelled_error());
    }
    snapshot_db_to(inputs.conn, &db_snapshot).await?;

    // ── Phase 2: build the ZIP payload (blocking) ────────────────────────
    let manifest_template = BackupManifest {
        format_version: BACKUP_FORMAT_VERSION,
        kind: BACKUP_KIND.to_string(),
        created_at: Utc::now().to_rfc3339(),
        app_version: inputs.app_version.to_string(),
        latest_migration: latest_migration_name(),
        runtime: inputs.runtime_label.to_string(),
        includes_external_transcripts: false, // set after packing
        includes_secrets: true,
        entries: Vec::new(),
    };

    let uploads_root = inputs.uploads_root.clone();
    let tokens_json = inputs.data_dir.join("tokens.json");
    let prefs_json = crate::paths::codeg_home_dir().join("preferences.json");
    let include_external = options.include_external_transcripts;

    let zip_tmp_c = zip_tmp.clone();
    let db_snapshot_c = db_snapshot.clone();
    let cancel_c = cancel.clone();
    let emitter_c = emitter.clone();
    let op_id_c = op_id.to_string();

    emit(emitter, op_id, BackupPhase::Archiving, 0, None, None);
    let manifest = tokio::task::spawn_blocking(move || -> Result<BackupManifest, AppCommandError> {
        let mut builder = ArchiveBuilder::create(&zip_tmp_c)?;
        let mut prog = |path: &str, processed: u64| {
            emit(
                &emitter_c,
                &op_id_c,
                BackupPhase::Archiving,
                processed,
                None,
                Some(path.to_string()),
            );
        };
        builder.add_file("db/codeg.db", &db_snapshot_c, &cancel_c, &mut prog)?;
        builder.add_dir(
            "uploads",
            &uploads_root,
            &is_excluded_upload,
            &cancel_c,
            &mut prog,
        )?;
        if tokens_json.is_file() {
            builder.add_file("tokens.json", &tokens_json, &cancel_c, &mut prog)?;
        }
        if prefs_json.is_file() {
            builder.add_file("preferences.json", &prefs_json, &cancel_c, &mut prog)?;
        }
        let mut manifest = manifest_template;
        let packed_external = if include_external {
            external::add_external_sources(&mut builder, &cancel_c, &mut prog)?
        } else {
            false
        };
        manifest.includes_external_transcripts = packed_external;
        builder.finish(manifest)
    })
    .await
    .map_err(|e| AppCommandError::task_execution_failed("Archive task failed").with_detail(e.to_string()))??;

    // ── Phase 3: deliver (encrypt or copy) into dest_path atomically ─────
    let part = with_part_suffix(dest_path);
    if let Some(parent) = dest_path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    match options.passphrase.as_deref().filter(|p| !p.is_empty()) {
        Some(pass) => {
            emit(emitter, op_id, BackupPhase::Encrypting, 0, None, None);
            let zip_tmp_c = zip_tmp.clone();
            let part_c = part.clone();
            let pass = pass.to_string();
            let cancel_c = cancel.clone();
            tokio::task::spawn_blocking(move || crypto::encrypt_file(&zip_tmp_c, &part_c, &pass, &cancel_c))
                .await
                .map_err(|e| AppCommandError::task_execution_failed("Encrypt task failed").with_detail(e.to_string()))??;
        }
        None => {
            tokio::fs::copy(&zip_tmp, &part)
                .await
                .map_err(super::map_disk_full)?;
        }
    }
    tokio::fs::rename(&part, dest_path).await.map_err(AppCommandError::io)?;

    let total = manifest.total_bytes();
    emit(emitter, op_id, BackupPhase::Done, total, Some(total), None);
    Ok(manifest)
}

/// Validate a candidate backup before applying it. Detects encryption, reads
/// (and thereby passphrase-verifies) the manifest, and checks version
/// compatibility — without touching live data.
pub(crate) async fn inspect_backup_core(
    src: &Path,
    passphrase: Option<&str>,
) -> Result<BackupPreview, AppCommandError> {
    let src_buf = src.to_path_buf();
    let encrypted =
        tokio::task::spawn_blocking(move || crypto::is_encrypted(&src_buf))
            .await
            .map_err(|e| AppCommandError::task_execution_failed("Inspect task failed").with_detail(e.to_string()))??;

    if encrypted && passphrase.is_none_or(|p| p.is_empty()) {
        return Ok(BackupPreview {
            encrypted: true,
            needs_passphrase: true,
            manifest: None,
            compatible: false,
            reject_reason: None,
        });
    }

    let (zip_path, _guard) = obtain_plaintext_zip(src, encrypted, passphrase).await?;
    let manifest =
        tokio::task::spawn_blocking(move || archive::read_manifest(&zip_path))
            .await
            .map_err(|e| AppCommandError::task_execution_failed("Inspect task failed").with_detail(e.to_string()))??;

    let (mut compatible, mut reject_reason) = evaluate_compat(&manifest);
    // Mirror the structural checks stage applies, so the preview never reports
    // "compatible" for a backup that stage will reject (missing db/codeg.db,
    // unsafe/duplicate manifest paths).
    if compatible && archive::validate_manifest(&manifest).is_err() {
        compatible = false;
        reject_reason = Some(BACKUP_I18N_KEY_UNKNOWN_FORMAT.to_string());
    }
    Ok(BackupPreview {
        encrypted,
        needs_passphrase: false,
        manifest: Some(manifest),
        compatible,
        reject_reason,
    })
}

/// Scan a backup for external transcript entries whose live target already
/// exists. Called only when the user opts to restore to original locations,
/// so the UI can surface conflicts before any write.
pub(crate) async fn scan_external_conflicts_core(
    src: &Path,
    passphrase: Option<&str>,
) -> Result<Vec<super::external::ExternalConflict>, AppCommandError> {
    let src_buf = src.to_path_buf();
    let encrypted = tokio::task::spawn_blocking(move || crypto::is_encrypted(&src_buf))
        .await
        .map_err(|e| AppCommandError::task_execution_failed("Scan task failed").with_detail(e.to_string()))??;
    let (zip_path, _guard) = obtain_plaintext_zip(src, encrypted, passphrase).await?;
    tokio::task::spawn_blocking(move || super::external::scan_external_conflicts(&zip_path))
        .await
        .map_err(|e| AppCommandError::task_execution_failed("Scan task failed").with_detail(e.to_string()))?
}

/// Run `VACUUM INTO` to produce a transactionally-consistent, defragmented
/// single-file copy of the live DB — sidesteps the WAL `-wal`/`-shm` sidecars.
pub(crate) async fn snapshot_db_to(
    conn: &DatabaseConnection,
    dest: &Path,
) -> Result<(), AppCommandError> {
    // VACUUM INTO requires the destination not to exist.
    if dest.exists() {
        tokio::fs::remove_file(dest).await.map_err(AppCommandError::io)?;
    }
    let dest_lit = dest.to_string_lossy().replace('\'', "''");
    let sql = format!("VACUUM INTO '{dest_lit}';");
    conn.execute(Statement::from_string(DbBackend::Sqlite, sql))
        .await
        .map_err(|e| AppCommandError::database_error("VACUUM INTO failed").with_detail(e.to_string()))?;
    Ok(())
}

/// Decrypt-to-temp if needed; returns a plaintext ZIP path plus a guard that
/// must outlive any read of that path.
pub(crate) async fn obtain_plaintext_zip(
    src: &Path,
    encrypted: bool,
    passphrase: Option<&str>,
) -> Result<(PathBuf, Option<tempfile::TempDir>), AppCommandError> {
    if !encrypted {
        return Ok((src.to_path_buf(), None));
    }
    let pass = passphrase.unwrap_or_default().to_string();
    let td = tempfile::tempdir().map_err(AppCommandError::io)?;
    let out = td.path().join("decrypted.zip");
    let src_c = src.to_path_buf();
    let out_c = out.clone();
    let cancel = CancellationToken::new();
    tokio::task::spawn_blocking(move || crypto::decrypt_file(&src_c, &out_c, &pass, &cancel))
        .await
        .map_err(|e| AppCommandError::task_execution_failed("Decrypt task failed").with_detail(e.to_string()))??;
    Ok((out, Some(td)))
}

/// `(compatible, reject_reason_i18n_key)`. Schema compatibility is keyed off
/// the migration identity (more robust than semver): an unknown
/// `latest_migration` means the backup is newer than this binary understands.
pub(crate) fn evaluate_compat(manifest: &BackupManifest) -> (bool, Option<String>) {
    if manifest.format_version > BACKUP_FORMAT_VERSION || manifest.kind != BACKUP_KIND {
        return (false, Some(BACKUP_I18N_KEY_UNKNOWN_FORMAT.to_string()));
    }
    if !known_migration(&manifest.latest_migration) {
        return (false, Some(BACKUP_I18N_KEY_NEWER_VERSION.to_string()));
    }
    (true, None)
}

fn latest_migration_name() -> String {
    Migrator::migrations()
        .last()
        .map(|m| m.name().to_string())
        .unwrap_or_default()
}

fn known_migration(name: &str) -> bool {
    Migrator::migrations().iter().any(|m| m.name() == name)
}

/// Exclude upload staging dirs (`uploads/.tmp/`) and any codeg-internal
/// `.codeg-*` directory (restore staging / safety snapshots) from the archive.
fn is_excluded_upload(rel: &Path) -> bool {
    rel.components().any(|c| match c {
        std::path::Component::Normal(s) => {
            let s = s.to_string_lossy();
            s == ".tmp" || s.starts_with(".codeg")
        }
        _ => false,
    })
}

fn with_part_suffix(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

fn emit(
    emitter: &EventEmitter,
    op_id: &str,
    phase: BackupPhase,
    processed: u64,
    total: Option<u64>,
    path: Option<String>,
) {
    emit_event(
        emitter,
        BACKUP_PROGRESS_EVENT,
        BackupProgress {
            op_id: op_id.to_string(),
            phase,
            processed_bytes: processed,
            total_bytes: total,
            current_path: path,
            error: None,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_disk_db;
    use sea_orm::Database;

    async fn count_folders(db_path: &Path) -> i64 {
        let url = format!("sqlite:{}?mode=ro", db_path.to_string_lossy());
        let conn = Database::connect(url).await.expect("open restored db");
        let row = conn
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) AS c FROM folder;".to_owned(),
            ))
            .await
            .expect("query")
            .expect("row");
        row.try_get::<i64>("", "c").expect("count")
    }

    fn inputs<'a>(conn: &'a DatabaseConnection, data_dir: &'a Path, uploads: PathBuf) -> BackupInputs<'a> {
        BackupInputs {
            conn,
            data_dir,
            uploads_root: uploads,
            app_version: "0.15.0",
            runtime_label: "server",
        }
    }

    #[tokio::test]
    async fn backup_roundtrip_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(dir.path()).await;
        crate::db::service::folder_service::add_folder(&db.conn, "/tmp/proj")
            .await
            .expect("seed folder");

        let uploads = dir.path().join("uploads");
        std::fs::create_dir_all(uploads.join(".tmp")).unwrap();
        std::fs::write(uploads.join("att.txt"), b"attachment").unwrap();
        std::fs::write(uploads.join(".tmp/partial"), b"should be skipped").unwrap();
        let dest = dir.path().join("backup.codeg.zip");

        let cancel = CancellationToken::new();
        let manifest = create_backup_core(
            inputs(&db.conn, dir.path(), uploads.clone()),
            BackupOptions::default(),
            &dest,
            &EventEmitter::Noop,
            "t1",
            &cancel,
        )
        .await
        .unwrap();

        assert!(dest.exists());
        assert!(manifest.entries.iter().any(|e| e.path == "db/codeg.db"));
        assert!(manifest.entries.iter().any(|e| e.path == "uploads/att.txt"));
        assert!(!manifest.entries.iter().any(|e| e.path.contains(".tmp")));

        // Inspect must report compatible (uses our own latest migration).
        let preview = inspect_backup_core(&dest, None).await.unwrap();
        assert!(!preview.encrypted);
        assert!(preview.compatible, "reject: {:?}", preview.reject_reason);

        // Extract and confirm the snapshot is a real DB carrying our row.
        let out = dir.path().join("out");
        archive::extract_all(&dest, &out, &manifest, &cancel, &mut archive::null_progress())
            .unwrap();
        assert_eq!(count_folders(&out.join("db/codeg.db")).await, 1);
    }

    #[tokio::test]
    async fn backup_roundtrip_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(dir.path()).await;
        let uploads = dir.path().join("uploads");
        std::fs::create_dir_all(&uploads).unwrap();
        let dest = dir.path().join("backup.codegbak");

        let cancel = CancellationToken::new();
        create_backup_core(
            inputs(&db.conn, dir.path(), uploads),
            BackupOptions {
                include_external_transcripts: false,
                passphrase: Some("s3cret".to_string()),
            },
            &dest,
            &EventEmitter::Noop,
            "t2",
            &cancel,
        )
        .await
        .unwrap();

        assert!(crypto::is_encrypted(&dest).unwrap());

        // No passphrase → preview is locked.
        let locked = inspect_backup_core(&dest, None).await.unwrap();
        assert!(locked.encrypted && locked.needs_passphrase && locked.manifest.is_none());

        // Wrong passphrase → authentication error.
        assert!(inspect_backup_core(&dest, Some("wrong")).await.is_err());

        // Correct passphrase → manifest readable + compatible.
        let unlocked = inspect_backup_core(&dest, Some("s3cret")).await.unwrap();
        assert!(unlocked.manifest.is_some());
        assert!(unlocked.compatible);
    }

    #[tokio::test]
    async fn backup_then_stage_then_apply_roundtrip() {
        use super::super::restore::{
            apply_pending_restore_with_paths, stage_restore_core, ExternalRestoreMode,
            RestoreApplied, PENDING_MARKER,
        };

        let src_dir = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(src_dir.path()).await;
        crate::db::service::folder_service::add_folder(&db.conn, "/tmp/proj-a")
            .await
            .unwrap();
        crate::db::service::folder_service::add_folder(&db.conn, "/tmp/proj-b")
            .await
            .unwrap();
        let uploads = src_dir.path().join("uploads");
        std::fs::create_dir_all(&uploads).unwrap();
        let dest = src_dir.path().join("backup.codeg.zip");

        let cancel = CancellationToken::new();
        create_backup_core(
            inputs(&db.conn, src_dir.path(), uploads),
            BackupOptions::default(),
            &dest,
            &EventEmitter::Noop,
            "b1",
            &cancel,
        )
        .await
        .unwrap();

        // Stage into a fresh, separate data dir.
        let restore_dir = tempfile::tempdir().unwrap();
        let staged = stage_restore_core(
            &dest,
            restore_dir.path(),
            None,
            ExternalRestoreMode::Skip,
            &EventEmitter::Noop,
            "r1",
            &cancel,
        )
        .await
        .unwrap();
        assert!(PathBuf::from(&staged.staging_dir).join("db/codeg.db").exists());
        assert!(restore_dir.path().join(PENDING_MARKER).is_file());

        // Apply on "startup" → live DB carries the two seeded folders. Inject
        // temp uploads/preferences paths so the test never touches ~/.codeg.
        let live_uploads = restore_dir.path().join("live-uploads");
        let live_prefs = restore_dir.path().join("live-prefs.json");
        let applied = apply_pending_restore_with_paths(
            restore_dir.path(),
            &live_uploads,
            &live_prefs,
        )
        .unwrap();
        assert!(matches!(applied, RestoreApplied::Applied { .. }));
        let db_name = crate::db::database_file_name();
        assert_eq!(count_folders(&restore_dir.path().join(db_name)).await, 2);
        assert!(!restore_dir.path().join(PENDING_MARKER).exists());
    }

    #[tokio::test]
    async fn restore_with_empty_uploads_clears_live_uploads() {
        // A backup whose uploads section is empty must REPLACE (clear) live
        // uploads, not merge — the prior file survives only in the safety
        // snapshot.
        use super::super::restore::{
            apply_pending_restore_with_paths, stage_restore_core, ExternalRestoreMode,
        };
        let src_dir = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(src_dir.path()).await;
        let empty_uploads = src_dir.path().join("uploads"); // exists, no files
        std::fs::create_dir_all(&empty_uploads).unwrap();
        let dest = src_dir.path().join("backup.codeg.zip");
        let cancel = CancellationToken::new();
        create_backup_core(
            inputs(&db.conn, src_dir.path(), empty_uploads),
            BackupOptions::default(),
            &dest,
            &EventEmitter::Noop,
            "e1",
            &cancel,
        )
        .await
        .unwrap();

        let restore_dir = tempfile::tempdir().unwrap();
        stage_restore_core(
            &dest,
            restore_dir.path(),
            None,
            ExternalRestoreMode::Skip,
            &EventEmitter::Noop,
            "e2",
            &cancel,
        )
        .await
        .unwrap();

        // Live uploads has a stale file that the backup does not contain.
        let live_uploads = restore_dir.path().join("live-uploads");
        std::fs::create_dir_all(&live_uploads).unwrap();
        std::fs::write(live_uploads.join("stale.png"), b"old").unwrap();

        apply_pending_restore_with_paths(
            restore_dir.path(),
            &live_uploads,
            &restore_dir.path().join("live-prefs.json"),
        )
        .unwrap();

        // Stale file is gone from live uploads (preserved only in the snapshot).
        assert!(!live_uploads.join("stale.png").exists());
    }

    fn manifest_with_migration(latest_migration: &str, format_version: u32) -> BackupManifest {
        BackupManifest {
            format_version,
            kind: BACKUP_KIND.to_string(),
            created_at: "2026-06-06T00:00:00Z".to_string(),
            app_version: "9.9.9".to_string(),
            latest_migration: latest_migration.to_string(),
            runtime: "server".to_string(),
            includes_external_transcripts: false,
            includes_secrets: true,
            entries: Vec::new(),
        }
    }

    #[test]
    fn evaluate_compat_gates_on_migration_and_format() {
        // A migration this binary knows → compatible.
        let known = latest_migration_name();
        let (ok, reason) = evaluate_compat(&manifest_with_migration(&known, BACKUP_FORMAT_VERSION));
        assert!(ok && reason.is_none());

        // A migration we don't know → newer version, rejected.
        let (ok, reason) =
            evaluate_compat(&manifest_with_migration("m99999999_000001_from_the_future", 1));
        assert!(!ok);
        assert_eq!(reason.as_deref(), Some(BACKUP_I18N_KEY_NEWER_VERSION));

        // A newer archive layout → unknown format.
        let (ok, reason) =
            evaluate_compat(&manifest_with_migration(&known, BACKUP_FORMAT_VERSION + 1));
        assert!(!ok);
        assert_eq!(reason.as_deref(), Some(BACKUP_I18N_KEY_UNKNOWN_FORMAT));
    }
}
